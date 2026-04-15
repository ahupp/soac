use pyo3::ffi;
use pyo3::prelude::*;
use std::ffi::c_int;
use std::ptr;

unsafe extern "C" {
    fn _PyEval_ImportFrom(
        tstate: *mut ffi::PyThreadState,
        module: *mut ffi::PyObject,
        name: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

pub fn import_module_level(
    py: Python<'_>,
    name: &Bound<'_, PyAny>,
    globals: &Bound<'_, PyAny>,
    fromlist: Option<&Bound<'_, PyAny>>,
    level: i32,
) -> PyResult<Py<PyAny>> {
    let fromlist = fromlist.map_or(ptr::null_mut(), Bound::as_ptr);
    let result = unsafe {
        ffi::PyImport_ImportModuleLevelObject(
            name.as_ptr(),
            globals.as_ptr(),
            globals.as_ptr(),
            fromlist,
            level as c_int,
        )
    };
    unsafe { Bound::from_owned_ptr_or_err(py, result).map(Bound::unbind) }
}

pub fn import_from(
    py: Python<'_>,
    module: &Bound<'_, PyAny>,
    name: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let result =
        unsafe { _PyEval_ImportFrom(ffi::PyThreadState_Get(), module.as_ptr(), name.as_ptr()) };
    unsafe { Bound::from_owned_ptr_or_err(py, result).map(Bound::unbind) }
}
