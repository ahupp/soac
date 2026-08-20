//! Independent native-build recipes and exact immutable code data.
//!
//! Recipes are body evidence, not function/environment identity, ownership, or
//! execution authority. Snapshots retain only Rust data; no module is executed.
//! All untrusted constant traversal uses exact native types, never Python
//! code-object/tuple equality or overridable attribute access.

use std::ffi::{c_int, c_uint, c_void};
use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;

use crate::strict_runtime_unavailable;

mod native_opcodes {
    include!(concat!(env!("OUT_DIR"), "/cpython_call_opcodes.rs"));
}

#[derive(Clone, Copy)]
#[repr(u32)]
pub(super) enum RecipeKind {
    Dataclasses = 1,
    Reprlib = 2,
}

impl RecipeKind {
    fn filename(self) -> &'static str {
        match self {
            Self::Dataclasses => "<frozen dataclasses>",
            Self::Reprlib => "<frozen reprlib>",
        }
    }
}

pub(super) use crate::code_view::{RawPySoacCodeView, view};

unsafe extern "C" {
    fn PySoac_GetDataclassRecipe(kind: c_uint) -> *mut ffi::PyObject;
    fn PyCode_GetCode(code: *mut ffi::PyCodeObject) -> *mut ffi::PyObject;
    fn PyLong_AsNativeBytes(
        value: *mut ffi::PyObject,
        buffer: *mut c_void,
        size: ffi::Py_ssize_t,
        flags: c_int,
    ) -> ffi::Py_ssize_t;
    fn _PySet_NextEntry(
        set: *mut ffi::PyObject,
        position: *mut ffi::Py_ssize_t,
        key: *mut *mut ffi::PyObject,
        hash: *mut ffi::Py_hash_t,
    ) -> c_int;
    fn PyCode_Addr2Location(
        code: *mut ffi::PyCodeObject,
        byte_offset: c_int,
        start_line: *mut c_int,
        start_column: *mut c_int,
        end_line: *mut c_int,
        end_column: *mut c_int,
    ) -> c_int;
}

/// Explicit protocol call expression, relative to the attested function's
/// first line. Locations select native call offsets only after full code
/// attestation; neither source locations nor opcode values confer authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CallSpan {
    pub(super) start_line: c_int,
    pub(super) end_line: c_int,
    pub(super) start_column: c_int,
    pub(super) end_column: c_int,
}

impl CallSpan {
    pub(super) const fn new(
        start_line: c_int,
        end_line: c_int,
        start_column: c_int,
        end_column: c_int,
    ) -> Self {
        Self {
            start_line,
            end_line,
            start_column,
            end_column,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CodeLayout {
    flags: c_int,
    argcount: c_int,
    posonlyargcount: c_int,
    kwonlyargcount: c_int,
    stacksize: c_int,
    firstlineno: c_int,
    nlocalsplus: c_int,
    framesize: c_int,
    nlocals: c_int,
    ncellvars: c_int,
    nfreevars: c_int,
    code_units: ffi::Py_ssize_t,
    strict_source_id: u64,
}

impl From<&RawPySoacCodeView> for CodeLayout {
    fn from(view: &RawPySoacCodeView) -> Self {
        Self {
            flags: view.flags,
            argcount: view.argcount,
            posonlyargcount: view.posonlyargcount,
            kwonlyargcount: view.kwonlyargcount,
            stacksize: view.stacksize,
            firstlineno: view.firstlineno,
            nlocalsplus: view.nlocalsplus,
            framesize: view.framesize,
            nlocals: view.nlocals,
            ncellvars: view.ncellvars,
            nfreevars: view.nfreevars,
            code_units: view.code_units,
            strict_source_id: view.strict_source_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Constant {
    None,
    Ellipsis,
    Bool(bool),
    Integer(Vec<u8>),
    Float(u64),
    Complex(u64, u64),
    Text(Vec<u32>),
    Bytes(Vec<u8>),
    Tuple(Vec<Constant>),
    FrozenSet(Vec<Constant>),
    Code(Box<CodeRecipe>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct CodeRecipe {
    layout: CodeLayout,
    bytecode: Vec<u8>,
    name: Vec<u32>,
    qualname: Vec<u32>,
    names: Vec<Vec<u32>>,
    localsplusnames: Vec<Vec<u32>>,
    localspluskinds: Vec<u8>,
    linetable: Vec<u8>,
    exceptiontable: Vec<u8>,
    constants: Vec<Constant>,
}

/// Read bindings from actual CPython callbacks during authenticated dataclass
/// construction. This does not reconstruct SOAC frames or expose frame locals.
/// Entry callbacks precede MAKE_CELL/COPY_FREE_VARS and use parameter_index
/// instead. Executing frames use this explicit cell/local classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameBinding {
    Local(usize),
    Cell(usize),
}

impl FrameBinding {
    fn from_kind(index: usize, kind: u8) -> Self {
        // CodeView ABI1: native pycore_code.h locals-plus kind bits.
        const CO_FAST_CELL: u8 = 0x40;
        const CO_FAST_FREE: u8 = 0x80;
        if kind & (CO_FAST_CELL | CO_FAST_FREE) != 0 {
            Self::Cell(index)
        } else {
            Self::Local(index)
        }
    }
}

/// Read a callback binding from the already authenticated single CPython
/// compiler result. This is construction-authentication ABI data, not a type
/// classification of the current value or a SOAC frame-layout requirement.
pub(super) fn compiled_binding(
    py: Python<'_>,
    code: *mut ffi::PyObject,
    index: usize,
) -> PyResult<Option<FrameBinding>> {
    let view = unsafe { view(py, code)? };
    if index >= view.nlocalsplus as usize
        || unsafe { ffi::PyBytes_CheckExact(view.localspluskinds) } == 0
        || unsafe { ffi::PyBytes_Size(view.localspluskinds) } != view.nlocalsplus as ffi::Py_ssize_t
    {
        return Ok(None);
    }
    let kinds = unsafe { ffi::PyBytes_AsString(view.localspluskinds) };
    Ok(Some(FrameBinding::from_kind(index, unsafe {
        *kinds.add(index) as u8
    })))
}

/// Reject hostile cycles/oversized constants before exhausting the Rust stack.
struct Budget {
    remaining: usize,
}

impl Budget {
    fn new() -> Self {
        Self { remaining: 1 << 22 }
    }

    fn take(&mut self, count: usize) -> bool {
        if let Some(remaining) = self.remaining.checked_sub(count) {
            self.remaining = remaining;
            true
        } else {
            false
        }
    }
}

unsafe fn text(value: *mut ffi::PyObject, budget: &mut Budget) -> Option<Vec<u32>> {
    if value.is_null() || unsafe { ffi::PyUnicode_CheckExact(value) } == 0 {
        return None;
    }
    let length = usize::try_from(unsafe { ffi::PyUnicode_GetLength(value) }).ok()?;
    if !budget.take(length) {
        return None;
    }
    Some(
        (0..length)
            .map(|index| unsafe { ffi::PyUnicode_ReadChar(value, index as ffi::Py_ssize_t) })
            .collect(),
    )
}

unsafe fn bytes(value: *mut ffi::PyObject, budget: &mut Budget) -> Option<Vec<u8>> {
    if value.is_null() || unsafe { ffi::PyBytes_CheckExact(value) } == 0 {
        return None;
    }
    let length = usize::try_from(unsafe { ffi::PyBytes_Size(value) }).ok()?;
    if !budget.take(length) {
        return None;
    }
    let data = unsafe { ffi::PyBytes_AsString(value) }.cast::<u8>();
    Some(unsafe { std::slice::from_raw_parts(data, length) }.to_vec())
}

unsafe fn text_tuple(value: *mut ffi::PyObject, budget: &mut Budget) -> Option<Vec<Vec<u32>>> {
    if value.is_null() || unsafe { ffi::PyTuple_CheckExact(value) } == 0 {
        return None;
    }
    let length = usize::try_from(unsafe { ffi::PyTuple_Size(value) }).ok()?;
    if !budget.take(length) {
        return None;
    }
    (0..length)
        .map(|index| unsafe {
            text(
                ffi::PyTuple_GetItem(value, index as ffi::Py_ssize_t),
                budget,
            )
        })
        .collect()
}

impl CodeRecipe {
    pub(super) fn load(py: Python<'_>, kind: RecipeKind) -> PyResult<Self> {
        let code = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(py, PySoac_GetDataclassRecipe(kind as c_uint))?
        };
        let filename = kind.filename().chars().map(u32::from).collect::<Vec<_>>();
        unsafe { Self::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0) }?
            .ok_or_else(|| strict_runtime_unavailable(py, "unsupported native stdlib recipe"))
    }

    /// Project only the one explicit module filename. Every nested code must
    /// use that same actual filename. Revalidate the function after this cold
    /// call: PyCode_GetCode may allocate an immutable bytecode cache.
    pub(super) fn matches(
        &self,
        py: Python<'_>,
        code: &Bound<'_, PyAny>,
        actual_filename: &[u32],
    ) -> PyResult<bool> {
        if unsafe { ffi::PyCode_Check(code.as_ptr()) } == 0 {
            return Ok(false);
        }
        Ok(
            unsafe { Self::capture(py, code.as_ptr(), actual_filename, &mut Budget::new(), 0)? }
                .as_ref()
                == Some(self),
        )
    }

    pub(super) fn filename(py: Python<'_>, code: &Bound<'_, PyAny>) -> PyResult<Option<Vec<u32>>> {
        if unsafe { ffi::PyCode_Check(code.as_ptr()) } == 0 {
            return Ok(None);
        }
        let view = unsafe { view(py, code.as_ptr())? };
        Ok(unsafe { text(view.filename, &mut Budget::new()) })
    }

    /// Used only after a live weak identity proves this is the exact code
    /// whose complete immutable graph was attested. It catches native source
    /// marking/layout changes without materializing bytecode in a callback.
    pub(super) fn matches_layout(
        &self,
        py: Python<'_>,
        code: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        if code.is_null() || unsafe { ffi::PyCode_Check(code) } == 0 {
            return Ok(false);
        }
        let view = unsafe { view(py, code)? };
        Ok(self.layout == CodeLayout::from(&view))
    }

    /// Resolve exactly one real CALL/CALL_KW/CALL_FUNCTION_EX in the immutable
    /// attested code. A LOAD/cache entry sharing that span cannot match. The
    /// runtime native frame boundary later rechecks this exact code-unit offset.
    pub(super) fn call_site(
        &self,
        py: Python<'_>,
        code: &Bound<'_, PyAny>,
        expected: CallSpan,
    ) -> PyResult<Option<usize>> {
        if !self.matches_layout(py, code.as_ptr())? {
            return Ok(None);
        }
        Ok(resolve_call_sites::<1>(
            code.as_ptr(),
            &self.bytecode,
            self.layout.firstlineno,
            expected,
        )
        .map(|sites| sites[0]))
    }

    pub(super) fn definition(&self, qualname: &str) -> Option<&Self> {
        self.find_definition(&qualname.chars().map(u32::from).collect::<Vec<_>>())
    }

    /// Select a nested code through an independently attested constant path,
    /// not through mutable Python attributes or a code-name search at entry.
    /// The path must be unique in the recipe. This is a cold operation; the
    /// caller retains only a weak witness after validating the actual parent.
    pub(super) fn definition_code<'py>(
        &self,
        py: Python<'py>,
        parent: &Bound<'py, PyAny>,
        qualname: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let name = qualname.chars().map(u32::from).collect::<Vec<_>>();
        let mut paths = Vec::new();
        self.definition_paths(&name, &mut Vec::new(), &mut paths);
        let [path] = paths.as_slice() else {
            return Ok(None);
        };
        let mut actual = parent.clone();
        for index in path {
            let view = unsafe { view(py, actual.as_ptr())? };
            if unsafe { ffi::PyTuple_CheckExact(view.consts) } == 0
                || *index >= unsafe { ffi::PyTuple_Size(view.consts) } as usize
            {
                return Ok(None);
            }
            let nested = unsafe { ffi::PyTuple_GetItem(view.consts, *index as ffi::Py_ssize_t) };
            if unsafe { ffi::PyCode_Check(nested) } == 0 {
                return Ok(None);
            }
            actual = unsafe { Bound::from_borrowed_ptr(py, nested) };
        }
        Ok(Some(actual))
    }

    fn definition_paths(
        &self,
        qualname: &[u32],
        prefix: &mut Vec<usize>,
        found: &mut Vec<Vec<usize>>,
    ) {
        if found.len() > 1 {
            return;
        }
        if self.qualname == qualname {
            found.push(prefix.clone());
        }
        for (index, value) in self.constants.iter().enumerate() {
            if let Constant::Code(code) = value {
                prefix.push(index);
                code.definition_paths(qualname, prefix, found);
                prefix.pop();
            }
        }
    }

    /// Resolve a binding while authenticating the actual CPython callback
    /// recipe. The index, not that spelling, crosses the native boundary;
    /// entry parameters and executed-frame cells remain distinct there.
    pub(super) fn local_index(&self, name: &str) -> Option<usize> {
        let mut found = None;
        for (index, actual) in self.localsplusnames.iter().enumerate() {
            if actual.iter().copied().eq(name.chars().map(u32::from)) {
                if found.is_some() {
                    return None;
                }
                found = Some(index);
            }
        }
        found
    }

    pub(super) fn parameter_index(&self, name: &str) -> Option<usize> {
        let index = self.local_index(name)?;
        (index < (self.layout.argcount + self.layout.kwonlyargcount) as usize).then_some(index)
    }

    pub(super) fn closure_index(&self, name: &str) -> Option<usize> {
        let first = self
            .localsplusnames
            .len()
            .checked_sub(self.layout.nfreevars as usize)?;
        self.local_index(name)?.checked_sub(first)
    }

    pub(super) fn closure_len(&self) -> usize {
        self.layout.nfreevars as usize
    }

    pub(super) fn executing_binding(&self, name: &str) -> Option<FrameBinding> {
        // CodeView ABI1 exposes CPython's pycore_code.h locals-plus kind
        // bytes. These are ABI bits, not a classification by variable spelling
        // or by the value currently occupying a frame slot.
        let index = self.local_index(name)?;
        let kind = *self.localspluskinds.get(index)?;
        Some(FrameBinding::from_kind(index, kind))
    }

    fn find_definition(&self, qualname: &[u32]) -> Option<&Self> {
        if self.qualname == qualname {
            return Some(self);
        }
        self.constants.iter().find_map(|value| match value {
            Constant::Code(code) => code.find_definition(qualname),
            _ => None,
        })
    }

    unsafe fn capture(
        py: Python<'_>,
        code: *mut ffi::PyObject,
        filename: &[u32],
        budget: &mut Budget,
        depth: usize,
    ) -> PyResult<Option<Self>> {
        if depth > 64 || !budget.take(1) {
            return Ok(None);
        }
        let view = unsafe { view(py, code)? };
        if unsafe { text(view.filename, budget) }.as_deref() != Some(filename)
            || unsafe { ffi::PyTuple_CheckExact(view.consts) } == 0
        {
            return Ok(None);
        }
        let Some(name) = (unsafe { text(view.name, budget) }) else {
            return Ok(None);
        };
        let Some(qualname) = (unsafe { text(view.qualname, budget) }) else {
            return Ok(None);
        };
        let Some(names) = (unsafe { text_tuple(view.names, budget) }) else {
            return Ok(None);
        };
        let Some(localsplusnames) = (unsafe { text_tuple(view.localsplusnames, budget) }) else {
            return Ok(None);
        };
        let Some(localspluskinds) = (unsafe { bytes(view.localspluskinds, budget) }) else {
            return Ok(None);
        };
        let Some(linetable) = (unsafe { bytes(view.linetable, budget) }) else {
            return Ok(None);
        };
        let Some(exceptiontable) = (unsafe { bytes(view.exceptiontable, budget) }) else {
            return Ok(None);
        };
        let bytecode =
            unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, PyCode_GetCode(code.cast()))? };
        let Some(bytecode) = (unsafe { bytes(bytecode.as_ptr(), budget) }) else {
            return Ok(None);
        };
        if usize::try_from(view.code_units)
            .ok()
            .and_then(|size| size.checked_mul(2))
            != Some(bytecode.len())
            || usize::try_from(view.nlocalsplus).ok() != Some(localsplusnames.len())
            || localsplusnames.len() != localspluskinds.len()
        {
            return Ok(None);
        }
        let count = usize::try_from(unsafe { ffi::PyTuple_Size(view.consts) })
            .expect("exact tuple length is nonnegative");
        if !budget.take(count) {
            return Ok(None);
        }
        let mut constants = Vec::with_capacity(count);
        for index in 0..count {
            let value = unsafe { ffi::PyTuple_GetItem(view.consts, index as ffi::Py_ssize_t) };
            let Some(value) = (unsafe { constant(py, value, filename, budget, depth + 1)? }) else {
                return Ok(None);
            };
            constants.push(value);
        }
        Ok(Some(Self {
            layout: CodeLayout::from(&view),
            bytecode,
            name,
            qualname,
            names,
            localsplusnames,
            localspluskinds,
            linetable,
            exceptiontable,
            constants,
        }))
    }
}

fn resolve_call_sites<const COUNT: usize>(
    code: *mut ffi::PyObject,
    bytecode: &[u8],
    first_line: c_int,
    expected: CallSpan,
) -> Option<[usize; COUNT]> {
    let mut found = [0; COUNT];
    let mut count = 0;
    for (offset, instruction) in bytecode.chunks_exact(2).enumerate() {
        if !matches!(
            instruction[0],
            native_opcodes::CALL | native_opcodes::CALL_KW | native_opcodes::CALL_FUNCTION_EX
        ) {
            continue;
        }
        let byte_offset = offset
            .checked_mul(2)
            .and_then(|offset| c_int::try_from(offset).ok())?;
        let (mut start_line, mut end_line, mut start_column, mut end_column) = (0, 0, 0, 0);
        if unsafe {
            PyCode_Addr2Location(
                code.cast(),
                byte_offset,
                &mut start_line,
                &mut start_column,
                &mut end_line,
                &mut end_column,
            )
        } == 0
        {
            return None;
        }
        let actual = CallSpan {
            start_line: start_line - first_line,
            end_line: end_line - first_line,
            start_column,
            end_column,
        };
        if actual == expected {
            if count == COUNT {
                return None;
            }
            found[count] = offset;
            count += 1;
        }
    }
    (count == COUNT).then_some(found)
}

/// Resolve a call in the one real compiler result after its exact source text
/// and native compile edge have been authenticated. This is not a code-origin
/// test and grants no role by itself. The native bridge materializes each code
/// node's immutable bytecode cache before its no-allocation compiled callback.
/// Borrowing that existing bytes object here does not compile or execute code.
pub(super) fn compiled_call_site(
    py: Python<'_>,
    code: *mut ffi::PyObject,
    expected: CallSpan,
) -> PyResult<Option<usize>> {
    Ok(compiled_call_sites::<1>(py, code, expected)?.map(|sites| sites[0]))
}

/// CPython assigns the same decorator-expression span to evaluating a
/// decorator factory and applying its result. The exact generated source
/// requires this ordered pair, not either matching CALL in isolation. The
/// caller separately authenticates each role/callee/argument/birth transition.
pub(super) fn compiled_decorator_calls(
    py: Python<'_>,
    code: *mut ffi::PyObject,
    expected: CallSpan,
) -> PyResult<Option<[usize; 2]>> {
    compiled_call_sites::<2>(py, code, expected)
}

fn compiled_call_sites<const COUNT: usize>(
    py: Python<'_>,
    code: *mut ffi::PyObject,
    expected: CallSpan,
) -> PyResult<Option<[usize; COUNT]>> {
    let layout = unsafe { view(py, code)? };
    let bytecode =
        unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, PyCode_GetCode(code.cast()))? };
    if unsafe { ffi::PyBytes_CheckExact(bytecode.as_ptr()) } == 0 {
        return Ok(None);
    }
    let mut data = ptr::null_mut();
    let mut size = 0;
    if unsafe { ffi::PyBytes_AsStringAndSize(bytecode.as_ptr(), &mut data, &mut size) } < 0 {
        return Err(PyErr::fetch(py));
    }
    if size < 0 || size % 2 != 0 || size / 2 != layout.code_units {
        return Ok(None);
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) };
    Ok(resolve_call_sites::<COUNT>(
        code,
        bytes,
        layout.firstlineno,
        expected,
    ))
}

unsafe fn constant(
    py: Python<'_>,
    value: *mut ffi::PyObject,
    filename: &[u32],
    budget: &mut Budget,
    depth: usize,
) -> PyResult<Option<Constant>> {
    if value.is_null() || depth > 64 || !budget.take(1) {
        return Ok(None);
    }
    Ok(Some(unsafe {
        if value == ffi::Py_None() {
            Constant::None
        } else if value == ffi::Py_Ellipsis() {
            Constant::Ellipsis
        } else if ffi::PyBool_Check(value) != 0 {
            Constant::Bool(value == ffi::Py_True())
        } else if ffi::PyLong_CheckExact(value) != 0 {
            // Signed little-endian bytes; no __index__ conversion. The size
            // may overestimate, so normalize redundant sign-extension bytes.
            let length = PyLong_AsNativeBytes(value, ptr::null_mut(), 0, 1);
            if length < 0 {
                return Err(PyErr::fetch(py));
            }
            if !budget.take(length as usize) {
                return Ok(None);
            }
            let mut digits = vec![0u8; length as usize];
            let written = PyLong_AsNativeBytes(value, digits.as_mut_ptr().cast(), length, 1);
            if written < 0 {
                return Err(PyErr::fetch(py));
            }
            if written > length {
                return Ok(None);
            }
            while digits.len() > 1 {
                let last = digits[digits.len() - 1];
                let sign = digits[digits.len() - 2] & 0x80;
                if (last == 0 && sign == 0) || (last == 0xff && sign != 0) {
                    digits.pop();
                } else {
                    break;
                }
            }
            Constant::Integer(digits)
        } else if ffi::PyFloat_CheckExact(value) != 0 {
            Constant::Float(ffi::PyFloat_AsDouble(value).to_bits())
        } else if ffi::PyComplex_CheckExact(value) != 0 {
            Constant::Complex(
                ffi::PyComplex_RealAsDouble(value).to_bits(),
                ffi::PyComplex_ImagAsDouble(value).to_bits(),
            )
        } else if ffi::PyUnicode_CheckExact(value) != 0 {
            let Some(value) = text(value, budget) else {
                return Ok(None);
            };
            Constant::Text(value)
        } else if ffi::PyBytes_CheckExact(value) != 0 {
            let Some(value) = bytes(value, budget) else {
                return Ok(None);
            };
            Constant::Bytes(value)
        } else if ffi::PyCode_Check(value) != 0 {
            let Some(code) = CodeRecipe::capture(py, value, filename, budget, depth + 1)? else {
                return Ok(None);
            };
            Constant::Code(Box::new(code))
        } else if ffi::PyTuple_CheckExact(value) != 0 {
            let count = ffi::PyTuple_Size(value);
            if !budget.take(count as usize) {
                return Ok(None);
            }
            let mut values = Vec::with_capacity(count as usize);
            for index in 0..count {
                let Some(value) = constant(
                    py,
                    ffi::PyTuple_GetItem(value, index),
                    filename,
                    budget,
                    depth + 1,
                )?
                else {
                    return Ok(None);
                };
                values.push(value);
            }
            Constant::Tuple(values)
        } else if ffi::PyFrozenSet_CheckExact(value) != 0 {
            let count = ffi::PySet_Size(value);
            if !budget.take(count as usize) {
                return Ok(None);
            }
            let mut values = Vec::with_capacity(count as usize);
            let mut position = 0;
            let mut key = ptr::null_mut();
            let mut hash = 0;
            while _PySet_NextEntry(value, &mut position, &mut key, &mut hash) != 0 {
                let Some(value) = constant(py, key, filename, budget, depth + 1)? else {
                    return Ok(None);
                };
                values.push(value);
            }
            values.sort_unstable(); // Rust structural order, not Python comparison.
            Constant::FrozenSet(values)
        } else {
            // Mutable containers, subclasses, and custom co_consts are not
            // native compiler constants. Never call their comparison methods.
            return Ok(None);
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use pyo3::types::{PyDict, PyModule, PyTuple};

    use super::*;

    #[test]
    fn generated_decorator_projection_requires_exactly_the_ordered_native_call_pair() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(py, c"def factory(__dataclasses_recursive_repr):\n @__dataclasses_recursive_repr()\n def __repr__(self):\n  return 'value'\n return __repr__\n", c"<generated decorator pair>", c"generated_decorator_pair").unwrap();
            let code = module
                .getattr("factory")
                .unwrap()
                .getattr("__code__")
                .unwrap();
            let filename = CodeRecipe::filename(py, &code).unwrap().unwrap();
            let recipe =
                unsafe { CodeRecipe::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0) }
                    .unwrap()
                    .unwrap();
            let span = CallSpan::new(1, 1, 2, 32);
            let pair = compiled_decorator_calls(py, code.as_ptr(), span)
                .unwrap()
                .unwrap();
            assert!(pair[0] < pair[1]);
            assert!(
                compiled_call_site(py, code.as_ptr(), span)
                    .unwrap()
                    .is_none()
            );
            assert!(
                compiled_decorator_calls(
                    py,
                    code.as_ptr(),
                    CallSpan {
                        end_column: 33,
                        ..span
                    }
                )
                .unwrap()
                .is_none()
            );

            let mut missing = recipe.bytecode.clone();
            missing[pair[1] * 2] = u8::MAX; // Remove one candidate; never execute this buffer.
            assert!(
                resolve_call_sites::<2>(code.as_ptr(), &missing, recipe.layout.firstlineno, span)
                    .is_none()
            );

            // Fault-inject a third CALL candidate at the first CALL's cache
            // location, which has the same native source span. This tests the
            // projection's exact cardinality; changed bytecode is not admitted
            // as a recipe/compiler result or executed by this fixture.
            let mut extra = recipe.bytecode.clone();
            extra[(pair[0] + 1) * 2] = native_opcodes::CALL;
            let three =
                resolve_call_sites::<3>(code.as_ptr(), &extra, recipe.layout.firstlineno, span)
                    .unwrap();
            assert_eq!(three, [pair[0], pair[0] + 1, pair[1]]);
            assert!(
                resolve_call_sites::<2>(code.as_ptr(), &extra, recipe.layout.firstlineno, span)
                    .is_none()
            );
        });
    }

    #[test]
    fn native_stdlib_recipes_match_selected_helper_bodies_without_module_execution() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            for (kind, module_name, definitions) in [
                (
                    RecipeKind::Dataclasses,
                    "dataclasses",
                    &[
                        "dataclass",
                        "_process_class",
                        "_FuncBuilder.add_fn",
                        "_FuncBuilder.add_fns_to_class",
                        "_field_init",
                        "_init_fn",
                        "_make_annotate_function",
                        "_frozen_get_del_attr",
                        "_set_new_attribute",
                    ][..],
                ),
                (RecipeKind::Reprlib, "reprlib", &["recursive_repr"][..]),
            ] {
                let recipe = CodeRecipe::load(py, kind).unwrap();
                // Import is the independent ordinary control, not how recipes
                // are obtained. The production loader only decodes native bytes.
                let module = PyModule::import(py, module_name).unwrap();
                let mut filename = None;
                for definition in definitions {
                    let mut actual = module.clone().into_any();
                    for name in definition.split('.') {
                        actual = actual.getattr(name).unwrap();
                    }
                    let code = actual.getattr("__code__").unwrap();
                    let actual_filename = filename
                        .get_or_insert_with(|| CodeRecipe::filename(py, &code).unwrap().unwrap());
                    assert!(
                        recipe
                            .definition(definition)
                            .unwrap()
                            .matches(py, &code, actual_filename)
                            .unwrap(),
                        "{definition}"
                    );
                }
            }
        });
    }

    #[test]
    fn code_recipe_snapshot_does_not_retain_the_decoded_python_code_tree() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let code = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    PySoac_GetDataclassRecipe(RecipeKind::Dataclasses as c_uint),
                )
            }
            .unwrap();
            let weak = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyWeakref_NewRef(code.as_ptr(), ptr::null_mut()),
                )
            }
            .unwrap();
            let filename = CodeRecipe::filename(py, &code).unwrap().unwrap();
            let snapshot =
                unsafe { CodeRecipe::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0) }
                    .unwrap()
                    .unwrap();
            drop(code);
            let mut referent = ptr::null_mut();
            let status = unsafe { ffi::PyWeakref_GetRef(weak.as_ptr(), &mut referent) };
            let referent = if referent.is_null() {
                None
            } else {
                Some(unsafe { Bound::<PyAny>::from_owned_ptr(py, referent) })
            };
            assert_eq!(status, 0);
            assert!(referent.is_none());
            assert!(snapshot.definition("dataclass").is_some());
        });
    }

    #[test]
    fn code_recipe_rejects_custom_constants_without_rich_comparison() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let source = CString::new(
                "events = []\nclass Trap:\n def __eq__(self, other):\n  events.append('eq')\n  return True\ndef sample():\n return 1\ntrap = Trap()\n",
            ).unwrap();
            let module =
                PyModule::from_code(py, &source, c"<constant recipe test>", c"code_recipe_test")
                    .unwrap();
            let code = module
                .getattr("sample")
                .unwrap()
                .getattr("__code__")
                .unwrap();
            let filename = CodeRecipe::filename(py, &code).unwrap().unwrap();
            let recipe =
                unsafe { CodeRecipe::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0) }
                    .unwrap()
                    .unwrap();
            let kwargs = PyDict::new(py);
            kwargs
                .set_item(
                    "co_consts",
                    PyTuple::new(
                        py,
                        [
                            module.getattr("trap").unwrap(),
                            1_i64.into_pyobject(py).unwrap().into_any(),
                        ],
                    )
                    .unwrap(),
                )
                .unwrap();
            let altered = code.call_method("replace", (), Some(&kwargs)).unwrap();
            assert!(!recipe.matches(py, &altered, &filename).unwrap());
            assert_eq!(module.getattr("events").unwrap().len().unwrap(), 0);
            assert!(recipe.matches(py, &code, &filename).unwrap());
        });
    }

    #[test]
    fn code_recipe_projects_only_a_consistent_filename_not_layout_or_nested_code() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let source = CString::new("def sample():\n return 1\ndef nested():\n def inner():\n  return 1\n return inner\n")
                .unwrap();
            let module =
                PyModule::from_code(py, &source, c"<original recipe test>", c"code_recipe_test")
                    .unwrap();
            for name in ["sample", "nested"] {
                let code = module.getattr(name).unwrap().getattr("__code__").unwrap();
                let filename = CodeRecipe::filename(py, &code).unwrap().unwrap();
                let recipe = unsafe {
                    CodeRecipe::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0)
                }
                .unwrap()
                .unwrap();
                let kwargs = PyDict::new(py);
                kwargs
                    .set_item("co_filename", "<projected recipe test>")
                    .unwrap();
                let altered = code.call_method("replace", (), Some(&kwargs)).unwrap();
                let projected = CodeRecipe::filename(py, &altered).unwrap().unwrap();
                assert_eq!(
                    recipe.matches(py, &altered, &projected).unwrap(),
                    name == "sample"
                );
                assert!(!recipe.matches(py, &altered, &filename).unwrap());
                kwargs.clear();
                kwargs.set_item("co_name", "different").unwrap();
                let altered = code.call_method("replace", (), Some(&kwargs)).unwrap();
                assert!(!recipe.matches(py, &altered, &filename).unwrap());
            }
        });
    }

    #[test]
    fn privileged_call_projection_ignores_non_call_instructions_with_the_same_span() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"def sample(callable, value):\n return callable(value)\n",
                c"<call projection test>",
                c"call_projection_test",
            )
            .unwrap();
            let code = module
                .getattr("sample")
                .unwrap()
                .getattr("__code__")
                .unwrap();
            let filename = CodeRecipe::filename(py, &code).unwrap().unwrap();
            let recipe =
                unsafe { CodeRecipe::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0) }
                    .unwrap()
                    .unwrap();
            let span = CallSpan::new(1, 1, 8, 23);
            let selected = recipe.call_site(py, &code, span).unwrap().unwrap();
            assert_eq!(recipe.bytecode[selected * 2], native_opcodes::CALL);
            let mut same_span_non_calls = 0;
            for (offset, instruction) in recipe.bytecode.chunks_exact(2).enumerate() {
                let (mut line, mut column, mut end_line, mut end_column) = (0, 0, 0, 0);
                let found = unsafe {
                    PyCode_Addr2Location(
                        code.as_ptr().cast(),
                        (offset * 2) as c_int,
                        &mut line,
                        &mut column,
                        &mut end_line,
                        &mut end_column,
                    )
                };
                if found != 0
                    && CallSpan::new(
                        line - recipe.layout.firstlineno,
                        end_line - recipe.layout.firstlineno,
                        column,
                        end_column,
                    ) == span
                    && !matches!(
                        instruction[0],
                        native_opcodes::CALL
                            | native_opcodes::CALL_KW
                            | native_opcodes::CALL_FUNCTION_EX
                    )
                {
                    same_span_non_calls += 1;
                }
            }
            assert!(
                same_span_non_calls > 0,
                "the fixture must exercise a shared instruction span"
            );
            assert_eq!(
                recipe
                    .call_site(py, &code, CallSpan::new(1, 1, 8, 16))
                    .unwrap(),
                None
            );
            assert_eq!(recipe.local_index("callable"), Some(0));
            assert_eq!(recipe.local_index("value"), Some(1));
            assert_eq!(recipe.local_index("unbound"), None);
        });
    }

    #[test]
    fn nested_template_selection_uses_a_unique_attested_constant_path() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"def outer():\n def inner(value):\n  return value\n return inner\n",
                c"<nested projection test>",
                c"nested_projection_test",
            )
            .unwrap();
            let function = module.getattr("outer").unwrap();
            let code = function.getattr("__code__").unwrap();
            let filename = CodeRecipe::filename(py, &code).unwrap().unwrap();
            let mut recipe =
                unsafe { CodeRecipe::capture(py, code.as_ptr(), &filename, &mut Budget::new(), 0) }
                    .unwrap()
                    .unwrap();
            let nested = recipe
                .definition_code(py, &code, "outer.<locals>.inner")
                .unwrap()
                .unwrap();
            assert!(nested.is(&function.call0().unwrap().getattr("__code__").unwrap()));
            assert!(
                recipe
                    .definition_code(py, &code, "another.<locals>.inner")
                    .unwrap()
                    .is_none()
            );
            let duplicate = recipe
                .constants
                .iter()
                .find(|constant| matches!(constant, Constant::Code(_)))
                .unwrap()
                .clone();
            recipe.constants.push(duplicate);
            assert!(
                recipe
                    .definition_code(py, &code, "outer.<locals>.inner")
                    .unwrap()
                    .is_none()
            );
        });
    }
}
