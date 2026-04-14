use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyModule, PyTuple};
use std::ffi::{CStr, CString, c_int};
use std::mem::{self, offset_of};
use std::ptr;

use super::{ModuleConstantId, ModuleConstantValue};

unsafe extern "C" {
    fn _Py_SetImmortal(op: *mut ffi::PyObject);
    fn PyUnstable_IsImmortal(op: *mut ffi::PyObject) -> c_int;
}

const SOAC_RUNTIME_BOOTSTRAP_HELPER_NAMES: &[&str] = &[
    "locals",
    "eval",
    "exec",
    "tuple_values",
    "make_function",
    "create_class",
    "import_",
    "import_attr",
    "class_lookup_global",
    "class_lookup_cell",
];

#[derive(Debug, Clone, Copy)]
pub(super) enum RuntimeNameConstantMode {
    ImportRuntime,
    BootstrapSoacRuntime,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum StaticPyObjectTemplate {
    CompactPyLongI64 { value: i64, digit: RawPyLongDigit },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaticPyObjectImage {
    pub bytes: Vec<u8>,
    pub align: u64,
    pub relocations: Vec<StaticPyObjectRelocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaticPyObjectRelocation {
    pub offset: u64,
    pub symbol: &'static str,
}

type RawPyLongDigit = u32;

#[repr(C)]
struct RawPyLongValue {
    lv_tag: usize,
    ob_digit: [RawPyLongDigit; 1],
}

#[repr(C)]
struct RawPyLongObject {
    ob_base: ffi::PyObject,
    long_value: RawPyLongValue,
}

const RAW_PYLONG_SHIFT: u32 = 30;
const RAW_PYLONG_MASK: i64 = (1_i64 << RAW_PYLONG_SHIFT) - 1;
const RAW_PYLONG_NON_SIZE_BITS: usize = 3;
const RAW_PYLONG_SIGN_POSITIVE: usize = 0;
const RAW_PYOBJECT_STATIC_IMMORTAL_FLAGS: i64 = (1 << 2) | (1 << 0);
const RAW_PYOBJECT_IMMORTAL_INITIAL_REFCNT_64: i64 = 3 << 30;
const RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64: i64 =
    RAW_PYOBJECT_IMMORTAL_INITIAL_REFCNT_64 | (RAW_PYOBJECT_STATIC_IMMORTAL_FLAGS << 48);
// Keep this conservative until the offline object-image path carries the CPython
// small-int range as validated build metadata.
const RAW_PYLONG_SMALL_INT_MAX: i64 = 1024;

impl StaticPyObjectTemplate {
    fn for_int(value: i64) -> Option<Self> {
        if !(RAW_PYLONG_SMALL_INT_MAX + 1..=RAW_PYLONG_MASK).contains(&value) {
            return None;
        }
        Some(Self::CompactPyLongI64 {
            value,
            digit: value as RawPyLongDigit,
        })
    }

    fn build_python_constant(self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if let Some(image) = self.static_object_image() {
            return materialize_static_pyobject_image(py, &image);
        }
        match self {
            Self::CompactPyLongI64 { value, .. } => build_pylong_i64_fallback(py, value),
        }
    }

    fn compact_pylong_lv_tag(self) -> usize {
        match self {
            Self::CompactPyLongI64 { .. } => {
                (1 << RAW_PYLONG_NON_SIZE_BITS) | RAW_PYLONG_SIGN_POSITIVE
            }
        }
    }

    fn static_object_image(self) -> Option<StaticPyObjectImage> {
        if !cfg!(all(
            target_arch = "x86_64",
            target_endian = "little",
            not(Py_GIL_DISABLED),
            not(py_sys_config = "Py_TRACE_REFS")
        )) {
            return None;
        }

        match self {
            Self::CompactPyLongI64 { digit, .. } => {
                let mut bytes = vec![0; mem::size_of::<RawPyLongObject>()];
                write_i64(
                    bytes.as_mut_slice(),
                    offset_of!(RawPyLongObject, ob_base) + offset_of!(ffi::PyObject, ob_refcnt),
                    RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64,
                );
                let type_offset =
                    offset_of!(RawPyLongObject, ob_base) + offset_of!(ffi::PyObject, ob_type);
                write_usize(
                    bytes.as_mut_slice(),
                    offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, lv_tag),
                    self.compact_pylong_lv_tag(),
                );
                write_u32(
                    bytes.as_mut_slice(),
                    offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, ob_digit),
                    digit,
                );
                Some(StaticPyObjectImage {
                    bytes,
                    align: mem::align_of::<RawPyLongObject>() as u64,
                    relocations: vec![StaticPyObjectRelocation {
                        offset: type_offset as u64,
                        symbol: "PyLong_Type",
                    }],
                })
            }
        }
    }
}

pub(super) fn static_pyobject_image(value: &ModuleConstantValue) -> Option<StaticPyObjectImage> {
    match value {
        ModuleConstantValue::Int(value) => {
            StaticPyObjectTemplate::for_int(*value)?.static_object_image()
        }
        ModuleConstantValue::Unicode(_)
        | ModuleConstantValue::Bytes(_)
        | ModuleConstantValue::BigInt(_)
        | ModuleConstantValue::FloatBits(_)
        | ModuleConstantValue::RuntimeName(_) => None,
    }
}

pub(super) fn build_python_constants(
    values: &[ModuleConstantValue],
    py: Python<'_>,
    runtime_name_mode: RuntimeNameConstantMode,
    mut static_resolver: impl FnMut(ModuleConstantId) -> PyResult<Option<*mut ffi::PyObject>>,
) -> PyResult<Vec<Py<PyAny>>> {
    let mut out = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let constant_id = ModuleConstantId(index);
        if static_pyobject_image(value).is_some()
            && let Some(ptr) = static_resolver(constant_id)?
        {
            let bound: Bound<'_, PyAny> = unsafe { Bound::from_borrowed_ptr(py, ptr) };
            out.push(bound.unbind());
            continue;
        }
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
                if let Some(template) = StaticPyObjectTemplate::for_int(*value) {
                    template.build_python_constant(py)?
                } else {
                    let ptr = unsafe { ffi::PyLong_FromLongLong(*value as std::ffi::c_longlong) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
            }
            ModuleConstantValue::BigInt(value) => {
                let value =
                    CString::new(value.as_str()).expect("big int literal should not contain NUL");
                let mut end_ptr = ptr::null_mut();
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

fn write_i64(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + mem::size_of::<i64>()].copy_from_slice(&value.to_le_bytes());
}

fn write_usize(bytes: &mut [u8], offset: usize, value: usize) {
    bytes[offset..offset + mem::size_of::<usize>()].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + mem::size_of::<u32>()].copy_from_slice(&value.to_le_bytes());
}

fn materialize_static_pyobject_image(
    py: Python<'_>,
    image: &StaticPyObjectImage,
) -> PyResult<Py<PyAny>> {
    let raw = unsafe { ffi::PyObject_Malloc(image.bytes.len()) } as *mut u8;
    if raw.is_null() {
        unsafe { ffi::PyErr_NoMemory() };
        return Err(PyErr::fetch(py));
    }
    let align = usize::try_from(image.align).unwrap_or(usize::MAX);
    if align == 0 || (raw as usize) % align != 0 {
        unsafe { ffi::PyObject_Free(raw.cast()) };
        return Err(PyRuntimeError::new_err(format!(
            "PyObject_Malloc returned memory {raw:p} that does not satisfy static object alignment {}",
            image.align
        )));
    }
    unsafe {
        ptr::copy_nonoverlapping(image.bytes.as_ptr(), raw, image.bytes.len());
    }
    if let Err(error) = unsafe {
        patch_static_pyobject_image_relocations(raw, image.bytes.len(), &image.relocations)
    } {
        unsafe { ffi::PyObject_Free(raw.cast()) };
        return Err(PyRuntimeError::new_err(error));
    }
    unsafe { Bound::from_owned_ptr_or_err(py, raw.cast::<ffi::PyObject>()).map(Py::from) }
}

fn build_pylong_i64_fallback(py: Python<'_>, value: i64) -> PyResult<Py<PyAny>> {
    let ptr = unsafe { ffi::PyLong_FromLongLong(value as std::ffi::c_longlong) };
    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
    Ok(bound.unbind())
}

unsafe fn patch_static_pyobject_image_relocations(
    base: *mut u8,
    len: usize,
    relocations: &[StaticPyObjectRelocation],
) -> Result<(), String> {
    for relocation in relocations {
        let offset = usize::try_from(relocation.offset).map_err(|_| {
            format!(
                "static PyObject relocation offset does not fit usize: {}",
                relocation.offset
            )
        })?;
        let end = offset.checked_add(mem::size_of::<usize>()).ok_or_else(|| {
            format!(
                "static PyObject relocation offset overflow at {}",
                relocation.offset
            )
        })?;
        if end > len {
            return Err(format!(
                "static PyObject relocation at byte {} exceeds image size {}",
                relocation.offset, len
            ));
        }
        let value = static_pyobject_relocation_value(relocation.symbol)?;
        ptr::copy_nonoverlapping(
            value.to_ne_bytes().as_ptr(),
            base.add(offset),
            mem::size_of::<usize>(),
        );
    }
    Ok(())
}

fn static_pyobject_relocation_value(symbol: &str) -> Result<usize, String> {
    match symbol {
        "PyLong_Type" => Ok(ptr::addr_of_mut!(ffi::PyLong_Type) as usize),
        _ => Err(format!(
            "unsupported static PyObject relocation symbol {symbol:?}"
        )),
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
_MISSING = object()

def _unsupported_frame_builtin(*args, **kwargs):
    raise NotImplementedError('soac.runtime does not support frame-sensitive locals/eval/exec')

locals = _unsupported_frame_builtin
eval = _unsupported_frame_builtin
# `exec` is a keyword, so expose soac.runtime.exec through the module dict.
globals()['exec'] = _unsupported_frame_builtin

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
    _builtins.exec('\\n'.join(outer_lines), {}, ns)
    code = ns['__dp_make_code']()
    if code.co_freevars != names:
        code = code.replace(co_freevars=names)
    _DP_CODE_WITH_FREEVARS_CACHE[cache_key] = code
    return code

def tuple_values(*values):
    return tuple(values)

make_function = _soac_ext.make_function

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
                try:
                    class_cell_value = class_cell.cell_contents
                except ValueError:
                    raise RuntimeError(
                        f'__class__ not set defining {name!r}; '
                        '__classcell__ propagated to type.__new__?'
                    )
                if class_cell_value is not cls:
                    raise TypeError(
                        f'__class__ set to {class_cell_value!r} defining {name!r} as {cls!r}'
                    )
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
    value = getattr(module, attr, _MISSING)
    if value is not _MISSING:
        return value
    module_name = getattr(module, '__name__', None)
    if module_name:
        submodule = _sys.modules.get(f'{module_name}.{attr}')
        if submodule is not None:
            return submodule
    module_spec = getattr(module, '__spec__', None)
    if (
        module_name
        and module_spec is not None
        and getattr(module_spec, '_initializing', False)
    ):
        message = (
            f'cannot import name {attr!r} from partially initialized module '
            f'{module_name!r} (most likely due to a circular import)'
        )
        raise ImportError(message, name=module_name) from None
    module_name = module_name or '<unknown module name>'
    module_file = getattr(module, '__file__', None)
    message = f'cannot import name {attr!r} from {module_name!r}'
    if module_file is not None:
        message = f'{message} ({module_file})'
    else:
        message = f'{message} (unknown location)'
    raise ImportError(message, name=module_name, path=module_file) from None

def class_lookup_global(class_ns, name, globals_dict):
    try:
        return class_ns[name]
    except KeyError:
        for type_param in class_ns.get('__type_params__', ()):
            if getattr(type_param, '__name__', None) == name:
                return type_param
        for member in class_ns.values():
            for type_param in getattr(member, '__type_params__', ()):
                if getattr(type_param, '__name__', None) == name:
                    return type_param
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
            let repr_text = CStr::from_ptr(repr_utf8).to_string_lossy();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_constants::ModuleCodegenConstants;

    #[test]
    fn static_pylong_template_accepts_only_positive_one_digit_non_small_ints() {
        assert_eq!(StaticPyObjectTemplate::for_int(-1), None);
        assert_eq!(StaticPyObjectTemplate::for_int(0), None);
        assert_eq!(
            StaticPyObjectTemplate::for_int(RAW_PYLONG_SMALL_INT_MAX),
            None
        );
        assert_eq!(
            StaticPyObjectTemplate::for_int(RAW_PYLONG_SMALL_INT_MAX + 1),
            Some(StaticPyObjectTemplate::CompactPyLongI64 {
                value: RAW_PYLONG_SMALL_INT_MAX + 1,
                digit: (RAW_PYLONG_SMALL_INT_MAX + 1) as RawPyLongDigit,
            })
        );
        assert_eq!(StaticPyObjectTemplate::for_int(RAW_PYLONG_MASK + 1), None);
    }

    #[test]
    fn build_python_constants_materializes_static_compact_pylong() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let constant_id = constants.intern_int(12345);
            let image = constants
                .static_pyobject_image(constant_id)
                .expect("test constant should have a static image");
            let objects = constants
                .build_python_constants(py)
                .expect("building static PyLong constant should succeed");
            let obj = objects[constant_id.0].as_ptr();

            unsafe {
                let mut expected_bytes = image.bytes.clone();
                patch_static_pyobject_image_relocations(
                    expected_bytes.as_mut_ptr(),
                    expected_bytes.len(),
                    &image.relocations,
                )
                .expect("test static image relocations should patch");
                let actual_bytes =
                    std::slice::from_raw_parts(obj.cast::<u8>(), expected_bytes.len());
                assert_eq!(actual_bytes, expected_bytes.as_slice());
                assert_ne!(ffi::PyLong_CheckExact(obj), 0);
                assert_eq!(ffi::PyLong_AsLongLong(obj), 12345);
                assert_ne!(PyUnstable_IsImmortal(obj), 0);

                let raw = &*(obj.cast::<RawPyLongObject>());
                assert_eq!(
                    raw.long_value.lv_tag,
                    StaticPyObjectTemplate::for_int(12345)
                        .expect("constant should be static-capable")
                        .compact_pylong_lv_tag()
                );
                assert_eq!(raw.long_value.ob_digit[0], 12345);
            }
        });
    }

    #[test]
    fn build_python_constants_uses_static_resolver_for_static_pylong() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let constant_id = constants.intern_int(12345);
            let resolved_ptr = unsafe { ffi::PyLong_FromLongLong(12345) };
            assert!(
                !resolved_ptr.is_null(),
                "test PyLong allocation should succeed"
            );

            let objects = constants
                .build_python_constants_with_static_resolver(py, false, |id| {
                    assert_eq!(id, constant_id);
                    Ok(Some(resolved_ptr))
                })
                .expect("building constants with static resolver should succeed");

            assert_eq!(objects[constant_id.0].as_ptr(), resolved_ptr);
        });
    }

    #[test]
    fn static_compact_pylong_image_matches_raw_layout() {
        let image = StaticPyObjectTemplate::for_int(12345)
            .expect("test constant should be static-capable")
            .static_object_image()
            .expect("test build should support static PyLong object images");
        let type_offset = offset_of!(RawPyLongObject, ob_base) + offset_of!(ffi::PyObject, ob_type);
        let tag_offset =
            offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, lv_tag);
        let digit_offset =
            offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, ob_digit);

        assert_eq!(image.bytes.len(), mem::size_of::<RawPyLongObject>());
        assert_eq!(image.align, mem::align_of::<RawPyLongObject>() as u64);
        assert_eq!(
            image.relocations,
            vec![StaticPyObjectRelocation {
                offset: type_offset as u64,
                symbol: "PyLong_Type",
            }]
        );
        assert_eq!(
            i64::from_le_bytes(
                image.bytes[offset_of!(RawPyLongObject, ob_base)
                    + offset_of!(ffi::PyObject, ob_refcnt)
                    ..offset_of!(RawPyLongObject, ob_base)
                        + offset_of!(ffi::PyObject, ob_refcnt)
                        + mem::size_of::<i64>()]
                    .try_into()
                    .expect("refcount slice should have i64 width")
            ),
            RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64
        );
        assert_eq!(
            usize::from_le_bytes(
                image.bytes[tag_offset..tag_offset + mem::size_of::<usize>()]
                    .try_into()
                    .expect("tag slice should have usize width")
            ),
            StaticPyObjectTemplate::for_int(12345)
                .expect("test constant should be static-capable")
                .compact_pylong_lv_tag()
        );
        assert_eq!(
            u32::from_le_bytes(
                image.bytes[digit_offset..digit_offset + mem::size_of::<u32>()]
                    .try_into()
                    .expect("digit slice should have u32 width")
            ),
            12345
        );
    }
}
