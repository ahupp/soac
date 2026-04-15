use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyModule;
use std::ffi::{CStr, CString, c_char, c_int};
use std::mem::{self, offset_of};
use std::ptr;

use super::{ModuleConstantId, ModuleConstantValue};

unsafe extern "C" {
    fn _Py_SetImmortal(op: *mut ffi::PyObject);
    fn PyUnstable_IsImmortal(op: *mut ffi::PyObject) -> c_int;
}

const SOAC_RUNTIME_BOOTSTRAP_HELPER_NAMES: &[&str] = &["_soac_ext", "import_", "import_attr"];

#[derive(Debug, Clone, Copy)]
pub(super) enum RuntimeNameConstantMode {
    ImportRuntime,
    BootstrapSoacRuntime,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum StaticPyObjectTemplate {
    PyLongI64 { value: i64 },
    PyLongDecimal { value: String },
    CompactUnicode { bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaticPyObjectImage {
    pub bytes: Vec<u8>,
    pub align: u64,
    pub writable: bool,
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

#[repr(C)]
struct RawPyASCIIObject {
    ob_base: ffi::PyObject,
    length: ffi::Py_ssize_t,
    hash: ffi::Py_hash_t,
    state: u32,
}

#[repr(C)]
struct RawPyCompactUnicodeObject {
    base: RawPyASCIIObject,
    utf8_length: ffi::Py_ssize_t,
    utf8: *mut c_char,
}

#[cfg(test)]
const RAW_PYLONG_SHIFT: u32 = 30;
const RAW_PYLONG_NON_SIZE_BITS: usize = 3;
const RAW_PYLONG_SMALL_INT_FLAG: usize = 1 << 2;
const RAW_PYOBJECT_STATIC_IMMORTAL_FLAGS: i64 = (1 << 2) | (1 << 0);
const RAW_PYOBJECT_IMMORTAL_INITIAL_REFCNT_64: i64 = 3 << 30;
const RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64: i64 =
    RAW_PYOBJECT_IMMORTAL_INITIAL_REFCNT_64 | (RAW_PYOBJECT_STATIC_IMMORTAL_FLAGS << 48);
const RAW_PYUNICODE_1BYTE_KIND: u32 = 1;
const RAW_PYUNICODE_2BYTE_KIND: u32 = 2;
const RAW_PYUNICODE_4BYTE_KIND: u32 = 4;
const RAW_PYUNICODE_KIND_SHIFT: u32 = 2;
const RAW_PYUNICODE_COMPACT_SHIFT: u32 = 5;
const RAW_PYUNICODE_ASCII_SHIFT: u32 = 6;
const RAW_PYUNICODE_COMPACT_MASK: u32 = 1 << RAW_PYUNICODE_COMPACT_SHIFT;
const RAW_PYUNICODE_ASCII_MASK: u32 = 1 << RAW_PYUNICODE_ASCII_SHIFT;
#[cfg(test)]
const RAW_PYUNICODE_INTERNED_MASK: u32 = 0b11;
const RAW_PYUNICODE_COMPACT_ASCII_STATE: u32 =
    raw_pyunicode_compact_state(RAW_PYUNICODE_1BYTE_KIND, true);

const fn raw_pyunicode_compact_state(kind: u32, ascii: bool) -> u32 {
    (kind << RAW_PYUNICODE_KIND_SHIFT)
        | (1 << RAW_PYUNICODE_COMPACT_SHIFT)
        | if ascii {
            1 << RAW_PYUNICODE_ASCII_SHIFT
        } else {
            0
        }
}

impl StaticPyObjectTemplate {
    fn for_int(value: i64) -> Option<Self> {
        Some(Self::PyLongI64 { value })
    }

    fn for_big_int(value: &str) -> Option<Self> {
        CString::new(value).ok()?;
        Some(Self::PyLongDecimal {
            value: value.to_string(),
        })
    }

    fn for_unicode(bytes: &[u8]) -> Option<Self> {
        ffi::Py_ssize_t::try_from(bytes.len()).ok()?;
        Some(Self::CompactUnicode {
            bytes: bytes.to_vec(),
        })
    }

    fn build_python_constant(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if let Some(image) = self.static_object_image() {
            return materialize_static_pyobject_image(py, &image);
        }
        match self {
            Self::PyLongI64 { value } => build_pylong_i64_fallback(py, *value),
            Self::PyLongDecimal { value } => build_pylong_decimal_fallback(py, value),
            Self::CompactUnicode { bytes } => build_unicode_constant(py, bytes).map(Py::from),
        }
    }

    fn static_object_image(&self) -> Option<StaticPyObjectImage> {
        if !cfg!(all(
            target_arch = "x86_64",
            target_endian = "little",
            not(Py_GIL_DISABLED),
            not(py_sys_config = "Py_TRACE_REFS")
        )) {
            return None;
        }

        match self {
            Self::PyLongI64 { value } => static_pylong_i64_image(*value),
            Self::PyLongDecimal { value } => static_pylong_decimal_image(value),
            Self::CompactUnicode { bytes: data } => static_compact_unicode_image(data),
        }
    }
}

pub(super) fn static_pyobject_image(value: &ModuleConstantValue) -> Option<StaticPyObjectImage> {
    match value {
        ModuleConstantValue::Int(value) => {
            StaticPyObjectTemplate::for_int(*value)?.static_object_image()
        }
        ModuleConstantValue::Unicode(bytes) => {
            StaticPyObjectTemplate::for_unicode(bytes)?.static_object_image()
        }
        ModuleConstantValue::BigInt(value) => {
            StaticPyObjectTemplate::for_big_int(value)?.static_object_image()
        }
        ModuleConstantValue::Bytes(_)
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
            if matches!(value, ModuleConstantValue::Unicode(_)) {
                out.push(unsafe { intern_borrowed_unicode_constant(py, ptr)? });
            } else {
                let bound: Bound<'_, PyAny> = unsafe { Bound::from_borrowed_ptr(py, ptr) };
                out.push(bound.unbind());
            }
            continue;
        }
        out.push(match value {
            ModuleConstantValue::Unicode(bytes) => {
                if let Some(template) = StaticPyObjectTemplate::for_unicode(bytes) {
                    build_static_unicode_constant(py, &template)?
                } else {
                    build_unicode_constant(py, bytes)?.unbind()
                }
            }
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
                if let Some(template) = StaticPyObjectTemplate::for_big_int(value) {
                    template.build_python_constant(py)?
                } else {
                    build_pylong_decimal_fallback(py, value)?
                }
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

fn write_py_ssize_t(bytes: &mut [u8], offset: usize, value: ffi::Py_ssize_t) {
    bytes[offset..offset + mem::size_of::<ffi::Py_ssize_t>()].copy_from_slice(&value.to_le_bytes());
}

fn write_py_hash_t(bytes: &mut [u8], offset: usize, value: ffi::Py_hash_t) {
    bytes[offset..offset + mem::size_of::<ffi::Py_hash_t>()].copy_from_slice(&value.to_le_bytes());
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

fn build_pylong_decimal_fallback(py: Python<'_>, value: &str) -> PyResult<Py<PyAny>> {
    let value = CString::new(value).expect("big int literal should not contain NUL");
    let mut end_ptr = ptr::null_mut();
    let ptr = unsafe { ffi::PyLong_FromString(value.as_ptr(), &mut end_ptr, 0) };
    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
    if !end_ptr.is_null() && unsafe { *end_ptr != 0 } {
        return Err(PyRuntimeError::new_err(
            "big int literal was only partially parsed by PyLong_FromString",
        ));
    }
    Ok(bound.unbind())
}

fn static_pylong_i64_image(value: i64) -> Option<StaticPyObjectImage> {
    static_pylong_image_from_c_api(|| {
        Some(unsafe { ffi::PyLong_FromLongLong(value as std::ffi::c_longlong) })
    })
}

fn static_pylong_decimal_image(value: &str) -> Option<StaticPyObjectImage> {
    let value = CString::new(value).ok()?;
    static_pylong_image_from_c_api(|| {
        let mut end_ptr = ptr::null_mut();
        let ptr = unsafe { ffi::PyLong_FromString(value.as_ptr(), &mut end_ptr, 0) };
        if ptr.is_null() {
            return Some(ptr);
        }
        if !end_ptr.is_null() && unsafe { *end_ptr != 0 } {
            unsafe { ffi::Py_DECREF(ptr) };
            return None;
        }
        Some(ptr)
    })
}

fn static_pylong_image_from_c_api(
    create: impl FnOnce() -> Option<*mut ffi::PyObject>,
) -> Option<StaticPyObjectImage> {
    Python::try_attach(|py| unsafe {
        let ptr = create()?;
        if ptr.is_null() {
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return None;
        }
        let object: Bound<'_, PyAny> = Bound::from_owned_ptr(py, ptr);
        static_pylong_image_from_borrowed_ptr(object.as_ptr())
    })
    .flatten()
}

unsafe fn static_pylong_image_from_borrowed_ptr(
    ptr: *mut ffi::PyObject,
) -> Option<StaticPyObjectImage> {
    if unsafe { ffi::PyLong_CheckExact(ptr) } == 0 {
        return None;
    }
    let raw = unsafe { &*(ptr.cast::<RawPyLongObject>()) };
    if raw.long_value.lv_tag & RAW_PYLONG_SMALL_INT_FLAG != 0 {
        return None;
    }
    let ndigits = raw.long_value.lv_tag >> RAW_PYLONG_NON_SIZE_BITS;
    let digit_count = ndigits.max(1);
    let digit_offset =
        offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, ob_digit);
    let digit_bytes = digit_count.checked_mul(mem::size_of::<RawPyLongDigit>())?;
    let object_size = digit_offset.checked_add(digit_bytes)?;
    let mut bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), object_size) }.to_vec();
    write_i64(
        bytes.as_mut_slice(),
        offset_of!(RawPyLongObject, ob_base) + offset_of!(ffi::PyObject, ob_refcnt),
        RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64,
    );
    let type_offset = offset_of!(RawPyLongObject, ob_base) + offset_of!(ffi::PyObject, ob_type);
    write_usize(bytes.as_mut_slice(), type_offset, 0);
    Some(StaticPyObjectImage {
        bytes,
        align: mem::align_of::<RawPyLongObject>() as u64,
        writable: false,
        relocations: vec![StaticPyObjectRelocation {
            offset: type_offset as u64,
            symbol: "PyLong_Type",
        }],
    })
}

fn static_compact_unicode_image(data: &[u8]) -> Option<StaticPyObjectImage> {
    if data.is_ascii() {
        return static_compact_ascii_unicode_image(data);
    }
    static_compact_unicode_image_from_utf8(data)
}

fn static_compact_ascii_unicode_image(data: &[u8]) -> Option<StaticPyObjectImage> {
    let len = ffi::Py_ssize_t::try_from(data.len()).ok()?;
    let object_size = mem::size_of::<RawPyASCIIObject>();
    let total_size = object_size.checked_add(data.len())?.checked_add(1)?;
    let mut bytes = vec![0; total_size];
    write_i64(
        bytes.as_mut_slice(),
        offset_of!(RawPyASCIIObject, ob_base) + offset_of!(ffi::PyObject, ob_refcnt),
        RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64,
    );
    let type_offset = offset_of!(RawPyASCIIObject, ob_base) + offset_of!(ffi::PyObject, ob_type);
    write_py_ssize_t(
        bytes.as_mut_slice(),
        offset_of!(RawPyASCIIObject, length),
        len,
    );
    write_py_hash_t(bytes.as_mut_slice(), offset_of!(RawPyASCIIObject, hash), -1);
    write_u32(
        bytes.as_mut_slice(),
        offset_of!(RawPyASCIIObject, state),
        RAW_PYUNICODE_COMPACT_ASCII_STATE,
    );
    bytes[object_size..object_size + data.len()].copy_from_slice(data);
    Some(StaticPyObjectImage {
        bytes,
        align: mem::align_of::<RawPyASCIIObject>() as u64,
        writable: true,
        relocations: vec![StaticPyObjectRelocation {
            offset: type_offset as u64,
            symbol: "PyUnicode_Type",
        }],
    })
}

fn static_compact_unicode_image_from_utf8(data: &[u8]) -> Option<StaticPyObjectImage> {
    let len = ffi::Py_ssize_t::try_from(data.len()).ok()?;
    Python::try_attach(|py| unsafe {
        let ptr = ffi::PyUnicode_DecodeUTF8(
            data.as_ptr().cast::<c_char>(),
            len,
            c"surrogatepass".as_ptr(),
        );
        if ptr.is_null() {
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return None;
        }
        let object: Bound<'_, PyAny> = Bound::from_owned_ptr(py, ptr);
        static_compact_unicode_image_from_borrowed_ptr(object.as_ptr())
    })
    .flatten()
}

unsafe fn static_compact_unicode_image_from_borrowed_ptr(
    ptr: *mut ffi::PyObject,
) -> Option<StaticPyObjectImage> {
    if unsafe { ffi::PyUnicode_CheckExact(ptr) } == 0 {
        return None;
    }
    if unsafe { ffi::PyUnicode_READY(ptr) } != 0 {
        if unsafe { !ffi::PyErr_Occurred().is_null() } {
            unsafe { ffi::PyErr_Clear() };
        }
        return None;
    }
    let state = unsafe { (&*(ptr.cast::<RawPyASCIIObject>())).state };
    if state & RAW_PYUNICODE_COMPACT_MASK == 0 || state & RAW_PYUNICODE_ASCII_MASK != 0 {
        return None;
    }

    let length = unsafe { ffi::PyUnicode_GET_LENGTH(ptr) };
    let char_count = usize::try_from(length).ok()?;
    let kind = unsafe { ffi::PyUnicode_KIND(ptr) };
    let char_size = match kind {
        RAW_PYUNICODE_1BYTE_KIND | RAW_PYUNICODE_2BYTE_KIND | RAW_PYUNICODE_4BYTE_KIND => {
            usize::try_from(kind).ok()?
        }
        _ => return None,
    };
    let data_ptr = unsafe { ffi::PyUnicode_DATA(ptr) }.cast::<u8>();
    if data_ptr.is_null() {
        return None;
    }

    let object_size = mem::size_of::<RawPyCompactUnicodeObject>();
    let data_size = char_count.checked_add(1)?.checked_mul(char_size)?;
    let total_size = object_size.checked_add(data_size)?;
    let mut bytes = vec![0; total_size];
    write_i64(
        bytes.as_mut_slice(),
        offset_of!(RawPyCompactUnicodeObject, base)
            + offset_of!(RawPyASCIIObject, ob_base)
            + offset_of!(ffi::PyObject, ob_refcnt),
        RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64,
    );
    let type_offset = offset_of!(RawPyCompactUnicodeObject, base)
        + offset_of!(RawPyASCIIObject, ob_base)
        + offset_of!(ffi::PyObject, ob_type);
    write_py_ssize_t(
        bytes.as_mut_slice(),
        offset_of!(RawPyCompactUnicodeObject, base) + offset_of!(RawPyASCIIObject, length),
        length,
    );
    write_py_hash_t(
        bytes.as_mut_slice(),
        offset_of!(RawPyCompactUnicodeObject, base) + offset_of!(RawPyASCIIObject, hash),
        -1,
    );
    write_u32(
        bytes.as_mut_slice(),
        offset_of!(RawPyCompactUnicodeObject, base) + offset_of!(RawPyASCIIObject, state),
        raw_pyunicode_compact_state(kind, false),
    );
    write_py_ssize_t(
        bytes.as_mut_slice(),
        offset_of!(RawPyCompactUnicodeObject, utf8_length),
        0,
    );
    write_usize(
        bytes.as_mut_slice(),
        offset_of!(RawPyCompactUnicodeObject, utf8),
        0,
    );
    unsafe {
        ptr::copy_nonoverlapping(data_ptr, bytes.as_mut_ptr().add(object_size), data_size);
    }
    Some(StaticPyObjectImage {
        bytes,
        align: mem::align_of::<RawPyCompactUnicodeObject>() as u64,
        writable: true,
        relocations: vec![StaticPyObjectRelocation {
            offset: type_offset as u64,
            symbol: "PyUnicode_Type",
        }],
    })
}

fn build_static_unicode_constant(
    py: Python<'_>,
    template: &StaticPyObjectTemplate,
) -> PyResult<Py<PyAny>> {
    intern_owned_unicode_constant(py, template.build_python_constant(py)?)
}

fn intern_owned_unicode_constant(py: Python<'_>, object: Py<PyAny>) -> PyResult<Py<PyAny>> {
    let mut ptr = object.into_ptr();
    unsafe {
        ffi::PyUnicode_InternInPlace(&mut ptr);
        Bound::from_owned_ptr_or_err(py, ptr).map(Py::from)
    }
}

unsafe fn intern_borrowed_unicode_constant(
    py: Python<'_>,
    ptr: *mut ffi::PyObject,
) -> PyResult<Py<PyAny>> {
    let mut ptr = ptr;
    unsafe {
        ffi::Py_INCREF(ptr);
        ffi::PyUnicode_InternInPlace(&mut ptr);
        Bound::from_owned_ptr_or_err(py, ptr).map(Py::from)
    }
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
        "PyUnicode_Type" => Ok(ptr::addr_of_mut!(ffi::PyUnicode_Type) as usize),
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
        "TRUE" | "FALSE" | "NONE" | "ELLIPSIS" | "EMPTY_TUPLE" | "ITER_COMPLETE" => {
            Ok(PyModule::import(py, "soac.bootstrap")?
                .getattr(name)?
                .unbind())
        }
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
import sys as _sys

def raise_deleted_name(name):
    raise UnboundLocalError(
        f'cannot access local variable {name!r} where it is not associated with a value'
    )

_MISSING = object()

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
    fn static_pylong_template_accepts_arbitrary_non_small_pylongs() {
        crate::initialize_test_python();
        assert_eq!(
            StaticPyObjectTemplate::for_int(-1)
                .expect("i64 PyLong templates should construct")
                .static_object_image(),
            None,
            "CPython small-int singletons should stay on the normal C API path"
        );
        assert_eq!(
            StaticPyObjectTemplate::for_int(0)
                .expect("i64 PyLong templates should construct")
                .static_object_image(),
            None,
            "CPython small-int singletons should stay on the normal C API path"
        );
        assert_eq!(
            StaticPyObjectTemplate::for_int(12345),
            Some(StaticPyObjectTemplate::PyLongI64 { value: 12345 })
        );
        assert!(
            StaticPyObjectTemplate::for_int(-12345)
                .expect("negative non-small int should be static-capable")
                .static_object_image()
                .is_some()
        );
        assert!(
            StaticPyObjectTemplate::for_int(1_i64 << RAW_PYLONG_SHIFT)
                .expect("multi-digit i64 int should be static-capable")
                .static_object_image()
                .is_some()
        );
        assert!(
            StaticPyObjectTemplate::for_big_int("123456789012345678901234567890")
                .expect("big int template should construct")
                .static_object_image()
                .is_some()
        );
    }

    #[test]
    fn static_unicode_template_accepts_ascii_and_non_ascii() {
        crate::initialize_test_python();
        assert_eq!(
            StaticPyObjectTemplate::for_unicode(b"ascii"),
            Some(StaticPyObjectTemplate::CompactUnicode {
                bytes: b"ascii".to_vec(),
            })
        );
        assert_eq!(
            StaticPyObjectTemplate::for_unicode("caf\u{e9}".as_bytes()),
            Some(StaticPyObjectTemplate::CompactUnicode {
                bytes: "caf\u{e9}".as_bytes().to_vec(),
            })
        );
        assert!(
            StaticPyObjectTemplate::for_unicode("caf\u{e9}".as_bytes())
                .expect("non-ASCII Unicode template should construct")
                .static_object_image()
                .is_some()
        );
    }

    #[test]
    fn build_python_constants_materializes_static_pylong() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let constant_id = constants.intern_int(1_i64 << RAW_PYLONG_SHIFT);
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
                assert_eq!(ffi::PyLong_AsLongLong(obj), 1_i64 << RAW_PYLONG_SHIFT);
                assert_ne!(PyUnstable_IsImmortal(obj), 0);

                let raw = &*(obj.cast::<RawPyLongObject>());
                assert_eq!(raw.long_value.lv_tag >> RAW_PYLONG_NON_SIZE_BITS, 2);
                let digits = raw.long_value.ob_digit.as_ptr();
                assert_eq!(*digits, 0);
                assert_eq!(*digits.add(1), 1);
            }
        });
    }

    #[test]
    fn build_python_constants_materializes_static_big_pylong() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let value = "123456789012345678901234567890";
            let constant_id = constants.intern(ModuleConstantValue::BigInt(value.to_string()));
            let image = constants
                .static_pyobject_image(constant_id)
                .expect("test big int constant should have a static image");
            let objects = constants
                .build_python_constants(py)
                .expect("building static big PyLong constant should succeed");
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
                assert_ne!(PyUnstable_IsImmortal(obj), 0);

                let expected = build_pylong_decimal_fallback(py, value)
                    .expect("test expected big int should build");
                assert_eq!(
                    ffi::PyObject_RichCompareBool(obj, expected.as_ptr(), ffi::Py_EQ),
                    1
                );
            }
        });
    }

    #[test]
    fn materialize_static_compact_ascii_unicode() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let image = StaticPyObjectTemplate::for_unicode(b"soac-static-ascii-can-intern")
                .expect("test constant should be static-capable")
                .static_object_image()
                .expect("test build should support static Unicode object images");
            let object = materialize_static_pyobject_image(py, &image)
                .expect("materializing static Unicode image should succeed");
            let obj = object.as_ptr();

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
                assert_ne!(ffi::PyUnicode_CheckExact(obj), 0);
                assert_eq!(ffi::_PyUnicode_CheckConsistency(obj, 1), 1);
                assert_eq!(ffi::PyUnicode_GET_LENGTH(obj), 28);
                assert_eq!(ffi::PyUnicode_KIND(obj), ffi::PyUnicode_1BYTE_KIND);
                let mut utf8_len = 0;
                let utf8 = ffi::PyUnicode_AsUTF8AndSize(obj, &mut utf8_len);
                assert!(!utf8.is_null());
                assert_eq!(utf8_len, 28);
                assert_eq!(
                    std::slice::from_raw_parts(utf8.cast::<u8>(), utf8_len as usize),
                    b"soac-static-ascii-can-intern"
                );

                let raw = &*(obj.cast::<RawPyASCIIObject>());
                assert_eq!(raw.hash, -1);
                let hash = ffi::PyObject_Hash(obj);
                assert_ne!(hash, -1);
                assert_eq!((&*(obj.cast::<RawPyASCIIObject>())).hash, hash);

                ffi::Py_INCREF(obj);
                let mut interned = obj;
                ffi::PyUnicode_InternInPlace(&mut interned);
                assert!(!interned.is_null());
                assert_ne!(ffi::PyUnicode_CheckExact(interned), 0);
                assert_eq!(ffi::PyUnicode_GET_LENGTH(interned), 28);
                ffi::Py_DECREF(interned);
            }
        });
    }

    #[test]
    fn build_python_constants_materializes_static_compact_ascii_unicode() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let constant_id = constants.intern_unicode_bytes(b"ascii");
            let image = constants
                .static_pyobject_image(constant_id)
                .expect("test constant should have a static image");
            let objects = constants
                .build_python_constants(py)
                .expect("building static Unicode constant should succeed");
            let obj = objects[constant_id.0].as_ptr();

            unsafe {
                assert_ne!(ffi::PyUnicode_CheckExact(obj), 0);
                assert_eq!(ffi::_PyUnicode_CheckConsistency(obj, 1), 1);
                let raw = &*(obj.cast::<RawPyASCIIObject>());
                assert_ne!(raw.state & RAW_PYUNICODE_INTERNED_MASK, 0);

                let image_object = materialize_static_pyobject_image(py, &image)
                    .expect("materializing static Unicode image should succeed");
                let image_obj = image_object.as_ptr();
                let mut expected_bytes = image.bytes.clone();
                patch_static_pyobject_image_relocations(
                    expected_bytes.as_mut_ptr(),
                    expected_bytes.len(),
                    &image.relocations,
                )
                .expect("test static image relocations should patch");
                let actual_bytes =
                    std::slice::from_raw_parts(image_obj.cast::<u8>(), expected_bytes.len());
                assert_eq!(actual_bytes, expected_bytes.as_slice());
            }
        });
    }

    #[test]
    fn build_python_constants_materializes_static_compact_non_ascii_unicode() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let value = "caf\u{e9} \u{1f40d}";
            let constant_id = constants.intern_unicode_bytes(value.as_bytes());
            let image = constants
                .static_pyobject_image(constant_id)
                .expect("test non-ASCII constant should have a static image");
            let objects = constants
                .build_python_constants(py)
                .expect("building static non-ASCII Unicode constant should succeed");
            let obj = objects[constant_id.0].as_ptr();

            unsafe {
                assert_ne!(ffi::PyUnicode_CheckExact(obj), 0);
                assert_eq!(ffi::_PyUnicode_CheckConsistency(obj, 1), 1);
                assert_eq!(ffi::PyUnicode_GET_LENGTH(obj), 6);
                assert_eq!(ffi::PyUnicode_KIND(obj), ffi::PyUnicode_4BYTE_KIND);
                let raw = &*(obj.cast::<RawPyCompactUnicodeObject>());
                assert_ne!(raw.base.state & RAW_PYUNICODE_COMPACT_MASK, 0);
                assert_eq!(raw.base.state & RAW_PYUNICODE_ASCII_MASK, 0);
                assert_ne!(raw.base.state & RAW_PYUNICODE_INTERNED_MASK, 0);

                let image_object = materialize_static_pyobject_image(py, &image)
                    .expect("materializing static non-ASCII Unicode image should succeed");
                let image_obj = image_object.as_ptr();
                let mut expected_bytes = image.bytes.clone();
                patch_static_pyobject_image_relocations(
                    expected_bytes.as_mut_ptr(),
                    expected_bytes.len(),
                    &image.relocations,
                )
                .expect("test static image relocations should patch");
                let actual_bytes =
                    std::slice::from_raw_parts(image_obj.cast::<u8>(), expected_bytes.len());
                assert_eq!(actual_bytes, expected_bytes.as_slice());

                let mut utf8_len = 0;
                let utf8 = ffi::PyUnicode_AsUTF8AndSize(image_obj, &mut utf8_len);
                assert!(!utf8.is_null());
                assert_eq!(utf8_len as usize, value.len());
                assert_eq!(
                    std::slice::from_raw_parts(utf8.cast::<u8>(), utf8_len as usize),
                    value.as_bytes()
                );
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
    fn build_python_constants_interns_static_resolver_unicode() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let constant_id = constants.intern_unicode_bytes(b"soac-static-resolver-ascii");
            let image = constants
                .static_pyobject_image(constant_id)
                .expect("test constant should have a static image");
            let resolved_object = materialize_static_pyobject_image(py, &image)
                .expect("materializing static Unicode image should succeed");
            let resolved_ptr = resolved_object.as_ptr();

            let objects = constants
                .build_python_constants_with_static_resolver(py, false, |id| {
                    assert_eq!(id, constant_id);
                    Ok(Some(resolved_ptr))
                })
                .expect("building constants with static resolver should succeed");

            let obj = objects[constant_id.0].as_ptr();
            unsafe {
                assert_ne!(ffi::PyUnicode_CheckExact(obj), 0);
                assert_eq!(ffi::_PyUnicode_CheckConsistency(obj, 1), 1);
                let raw = &*(obj.cast::<RawPyASCIIObject>());
                assert_ne!(raw.state & RAW_PYUNICODE_INTERNED_MASK, 0);
                assert_eq!(ffi::PyUnicode_GET_LENGTH(obj), 26);
                let mut utf8_len = 0;
                let utf8 = ffi::PyUnicode_AsUTF8AndSize(obj, &mut utf8_len);
                assert!(!utf8.is_null());
                assert_eq!(utf8_len, 26);
                assert_eq!(
                    std::slice::from_raw_parts(utf8.cast::<u8>(), utf8_len as usize),
                    b"soac-static-resolver-ascii"
                );
            }
        });
    }

    #[test]
    fn build_python_constants_interns_static_resolver_non_ascii_unicode() {
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut constants = ModuleCodegenConstants::default();
            let value = "caf\u{e9} \u{1f40d}";
            let constant_id = constants.intern_unicode_bytes(value.as_bytes());
            let image = constants
                .static_pyobject_image(constant_id)
                .expect("test non-ASCII constant should have a static image");
            let resolved_object = materialize_static_pyobject_image(py, &image)
                .expect("materializing static non-ASCII Unicode image should succeed");
            let resolved_ptr = resolved_object.as_ptr();

            let objects = constants
                .build_python_constants_with_static_resolver(py, false, |id| {
                    assert_eq!(id, constant_id);
                    Ok(Some(resolved_ptr))
                })
                .expect("building constants with static resolver should succeed");

            let obj = objects[constant_id.0].as_ptr();
            unsafe {
                assert_ne!(ffi::PyUnicode_CheckExact(obj), 0);
                assert_eq!(ffi::_PyUnicode_CheckConsistency(obj, 1), 1);
                let raw = &*(obj.cast::<RawPyCompactUnicodeObject>());
                assert_ne!(raw.base.state & RAW_PYUNICODE_COMPACT_MASK, 0);
                assert_eq!(raw.base.state & RAW_PYUNICODE_ASCII_MASK, 0);
                assert_ne!(raw.base.state & RAW_PYUNICODE_INTERNED_MASK, 0);
                assert_eq!(ffi::PyUnicode_GET_LENGTH(obj), 6);
                let mut utf8_len = 0;
                let utf8 = ffi::PyUnicode_AsUTF8AndSize(obj, &mut utf8_len);
                assert!(!utf8.is_null());
                assert_eq!(utf8_len as usize, value.len());
                assert_eq!(
                    std::slice::from_raw_parts(utf8.cast::<u8>(), utf8_len as usize),
                    value.as_bytes()
                );
            }
        });
    }

    #[test]
    fn static_pylong_image_matches_raw_layout() {
        crate::initialize_test_python();
        let image = StaticPyObjectTemplate::for_int(12345)
            .expect("test constant should be static-capable")
            .static_object_image()
            .expect("test build should support static PyLong object images");
        let type_offset = offset_of!(RawPyLongObject, ob_base) + offset_of!(ffi::PyObject, ob_type);
        let tag_offset =
            offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, lv_tag);
        let digit_offset =
            offset_of!(RawPyLongObject, long_value) + offset_of!(RawPyLongValue, ob_digit);

        assert_eq!(
            image.bytes.len(),
            digit_offset + mem::size_of::<RawPyLongDigit>()
        );
        assert_eq!(image.align, mem::align_of::<RawPyLongObject>() as u64);
        assert!(!image.writable);
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
            1 << RAW_PYLONG_NON_SIZE_BITS
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

    #[test]
    fn static_compact_ascii_unicode_image_matches_raw_layout() {
        let image = StaticPyObjectTemplate::for_unicode(b"ascii")
            .expect("test constant should be static-capable")
            .static_object_image()
            .expect("test build should support static Unicode object images");
        let type_offset =
            offset_of!(RawPyASCIIObject, ob_base) + offset_of!(ffi::PyObject, ob_type);
        let length_offset = offset_of!(RawPyASCIIObject, length);
        let hash_offset = offset_of!(RawPyASCIIObject, hash);
        let state_offset = offset_of!(RawPyASCIIObject, state);
        let data_offset = mem::size_of::<RawPyASCIIObject>();

        assert_eq!(image.bytes.len(), data_offset + b"ascii".len() + 1);
        assert_eq!(image.align, mem::align_of::<RawPyASCIIObject>() as u64);
        assert!(image.writable);
        assert_eq!(
            image.relocations,
            vec![StaticPyObjectRelocation {
                offset: type_offset as u64,
                symbol: "PyUnicode_Type",
            }]
        );
        assert_eq!(
            i64::from_le_bytes(
                image.bytes[offset_of!(RawPyASCIIObject, ob_base)
                    + offset_of!(ffi::PyObject, ob_refcnt)
                    ..offset_of!(RawPyASCIIObject, ob_base)
                        + offset_of!(ffi::PyObject, ob_refcnt)
                        + mem::size_of::<i64>()]
                    .try_into()
                    .expect("refcount slice should have i64 width")
            ),
            RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64
        );
        assert_eq!(
            ffi::Py_ssize_t::from_le_bytes(
                image.bytes[length_offset..length_offset + mem::size_of::<ffi::Py_ssize_t>()]
                    .try_into()
                    .expect("length slice should have Py_ssize_t width")
            ),
            5
        );
        assert_eq!(
            ffi::Py_hash_t::from_le_bytes(
                image.bytes[hash_offset..hash_offset + mem::size_of::<ffi::Py_hash_t>()]
                    .try_into()
                    .expect("hash slice should have Py_hash_t width")
            ),
            -1
        );
        assert_eq!(
            u32::from_le_bytes(
                image.bytes[state_offset..state_offset + mem::size_of::<u32>()]
                    .try_into()
                    .expect("state slice should have u32 width")
            ),
            RAW_PYUNICODE_COMPACT_ASCII_STATE
        );
        assert_eq!(
            &image.bytes[data_offset..data_offset + b"ascii".len() + 1],
            b"ascii\0"
        );
    }

    #[test]
    fn static_compact_non_ascii_unicode_image_matches_raw_layout() {
        crate::initialize_test_python();
        let image = StaticPyObjectTemplate::for_unicode("caf\u{e9} \u{1f40d}".as_bytes())
            .expect("test constant should be static-capable")
            .static_object_image()
            .expect("test build should support static non-ASCII Unicode object images");
        let type_offset = offset_of!(RawPyCompactUnicodeObject, base)
            + offset_of!(RawPyASCIIObject, ob_base)
            + offset_of!(ffi::PyObject, ob_type);
        let length_offset =
            offset_of!(RawPyCompactUnicodeObject, base) + offset_of!(RawPyASCIIObject, length);
        let hash_offset =
            offset_of!(RawPyCompactUnicodeObject, base) + offset_of!(RawPyASCIIObject, hash);
        let state_offset =
            offset_of!(RawPyCompactUnicodeObject, base) + offset_of!(RawPyASCIIObject, state);
        let utf8_length_offset = offset_of!(RawPyCompactUnicodeObject, utf8_length);
        let utf8_offset = offset_of!(RawPyCompactUnicodeObject, utf8);
        let data_offset = mem::size_of::<RawPyCompactUnicodeObject>();
        let char_count = 6;
        let char_size = mem::size_of::<u32>();
        let data_size = (char_count + 1) * char_size;

        assert_eq!(image.bytes.len(), data_offset + data_size);
        assert_eq!(
            image.align,
            mem::align_of::<RawPyCompactUnicodeObject>() as u64
        );
        assert!(image.writable);
        assert_eq!(
            image.relocations,
            vec![StaticPyObjectRelocation {
                offset: type_offset as u64,
                symbol: "PyUnicode_Type",
            }]
        );
        assert_eq!(
            i64::from_le_bytes(
                image.bytes[offset_of!(RawPyCompactUnicodeObject, base)
                    + offset_of!(RawPyASCIIObject, ob_base)
                    + offset_of!(ffi::PyObject, ob_refcnt)
                    ..offset_of!(RawPyCompactUnicodeObject, base)
                        + offset_of!(RawPyASCIIObject, ob_base)
                        + offset_of!(ffi::PyObject, ob_refcnt)
                        + mem::size_of::<i64>()]
                    .try_into()
                    .expect("refcount slice should have i64 width")
            ),
            RAW_PYOBJECT_STATIC_IMMORTAL_INITIAL_REFCNT_64
        );
        assert_eq!(
            ffi::Py_ssize_t::from_le_bytes(
                image.bytes[length_offset..length_offset + mem::size_of::<ffi::Py_ssize_t>()]
                    .try_into()
                    .expect("length slice should have Py_ssize_t width")
            ),
            char_count as ffi::Py_ssize_t
        );
        assert_eq!(
            ffi::Py_hash_t::from_le_bytes(
                image.bytes[hash_offset..hash_offset + mem::size_of::<ffi::Py_hash_t>()]
                    .try_into()
                    .expect("hash slice should have Py_hash_t width")
            ),
            -1
        );
        assert_eq!(
            u32::from_le_bytes(
                image.bytes[state_offset..state_offset + mem::size_of::<u32>()]
                    .try_into()
                    .expect("state slice should have u32 width")
            ),
            raw_pyunicode_compact_state(RAW_PYUNICODE_4BYTE_KIND, false)
        );
        assert_eq!(
            ffi::Py_ssize_t::from_le_bytes(
                image.bytes
                    [utf8_length_offset..utf8_length_offset + mem::size_of::<ffi::Py_ssize_t>()]
                    .try_into()
                    .expect("utf8 length slice should have Py_ssize_t width")
            ),
            0
        );
        assert_eq!(
            usize::from_le_bytes(
                image.bytes[utf8_offset..utf8_offset + mem::size_of::<usize>()]
                    .try_into()
                    .expect("utf8 pointer slice should have pointer width")
            ),
            0
        );
        let data = &image.bytes[data_offset..data_offset + data_size];
        assert_eq!(
            u32::from_le_bytes(data[0..4].try_into().expect("codepoint width")),
            'c' as u32
        );
        assert_eq!(
            u32::from_le_bytes(data[12..16].try_into().expect("codepoint width")),
            '\u{e9}' as u32
        );
        assert_eq!(
            u32::from_le_bytes(data[20..24].try_into().expect("codepoint width")),
            '\u{1f40d}' as u32
        );
        assert_eq!(
            u32::from_le_bytes(data[24..28].try_into().expect("terminator width")),
            0
        );
    }
}
