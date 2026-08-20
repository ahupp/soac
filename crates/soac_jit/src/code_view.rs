//! ABI-shaped borrowed view of one exact native code object.
//!
//! This view is immutable layout data, not source admission or callable
//! authority. The caller must pin the exact code while using its pointers.

use std::ffi::{c_int, c_uint};
use std::mem::{MaybeUninit, size_of};

use pyo3::ffi;
use pyo3::prelude::*;

use crate::strict_runtime_unavailable;

/// Public native view, not a mirror of PyCodeObject's private layout. All
/// pointers are borrowed while the exact code is pinned. Native validates size.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct RawPySoacCodeView {
    pub(crate) abi_version: c_uint,
    pub(crate) flags: c_int,
    pub(crate) argcount: c_int,
    pub(crate) posonlyargcount: c_int,
    pub(crate) kwonlyargcount: c_int,
    pub(crate) stacksize: c_int,
    pub(crate) firstlineno: c_int,
    pub(crate) nlocalsplus: c_int,
    pub(crate) framesize: c_int,
    pub(crate) nlocals: c_int,
    pub(crate) ncellvars: c_int,
    pub(crate) nfreevars: c_int,
    pub(crate) code_units: ffi::Py_ssize_t,
    pub(crate) strict_source_id: u64,
    pub(crate) consts: *mut ffi::PyObject,
    pub(crate) names: *mut ffi::PyObject,
    pub(crate) localsplusnames: *mut ffi::PyObject,
    pub(crate) localspluskinds: *mut ffi::PyObject,
    pub(crate) filename: *mut ffi::PyObject,
    pub(crate) name: *mut ffi::PyObject,
    pub(crate) qualname: *mut ffi::PyObject,
    pub(crate) linetable: *mut ffi::PyObject,
    pub(crate) exceptiontable: *mut ffi::PyObject,
}

unsafe extern "C" {
    fn PySoac_GetCodeView(
        code: *mut ffi::PyObject,
        view: *mut RawPySoacCodeView,
        size: usize,
    ) -> c_int;
}

pub(crate) unsafe fn view(py: Python<'_>, code: *mut ffi::PyObject) -> PyResult<RawPySoacCodeView> {
    let mut view = MaybeUninit::uninit();
    if unsafe { PySoac_GetCodeView(code, view.as_mut_ptr(), size_of::<RawPySoacCodeView>()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    let view = unsafe { view.assume_init() };
    if view.abi_version != 1 {
        return Err(strict_runtime_unavailable(
            py,
            "unsupported native code-view ABI",
        ));
    }
    Ok(view)
}
