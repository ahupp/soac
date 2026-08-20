//! Native dataclass ABI4. This module is registered only with the matching
//! managed CPython generation; no dynamic symbol lookup or compatibility ABI.

use std::ffi::{c_int, c_uint};
use std::ptr::NonNull;

use pyo3::ffi;
use pyo3::prelude::*;

use crate::strict_runtime_unavailable;

use super::code::{CodeRecipe, FrameBinding};

pub(super) const ROOT_FACTORY: c_uint = 1;
pub(super) const ROOT_APPLY: c_uint = 2;
pub(super) const GENERATED_EXEC: c_uint = 3;
pub(super) const SOURCE: c_uint = 1;
pub(super) const EXEC: c_uint = 2;
pub(super) const MEMBER: c_uint = 3;
pub(super) const BUILTIN_EXEC: c_uint = 4;
pub(super) const BUILTIN_SETATTR: c_uint = 5;
pub(super) const NEW_SLOTS: c_uint = 8;
pub(super) const CALLBACKS_ABI: c_uint = 4;
pub(super) const FUNCTION_MEMBER: c_uint = 1;
pub(super) const FROZEN_SETATTR: c_uint = 2;
pub(super) const FROZEN_DELATTR: c_uint = 3;
pub(super) const DECORATOR: c_uint = 256;
pub(super) const GENERATED_FACTORY: c_uint = 257;
pub(super) const ANNOTATION_PROVIDER: c_uint = 258;
pub(super) const REPR_IMPLEMENTATION: c_uint = 259;
pub(super) const COMPONENT_ANNOTATE: c_uint = 1;
pub(super) const COMPONENT_REPR: c_uint = 2;

#[repr(C)]
pub(crate) struct RawFrameView {
    _private: [u8; 0],
}

#[repr(C)]
pub(super) struct Callbacks {
    pub(super) abi_version: c_uint,
    pub(super) enter: unsafe extern "C" fn(
        *mut ffi::PyObject,
        c_uint,
        *const RawFrameView,
        *const RawFrameView,
        *mut c_uint,
    ) -> c_int,
    pub(super) create: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawFrameView,
        *mut ffi::PyObject,
        *mut c_uint,
    ) -> c_int,
    pub(super) validate_member: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        c_uint,
    ) -> c_int,
    pub(super) bridge: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawFrameView,
        *mut ffi::PyObject,
        c_uint,
        *const *mut ffi::PyObject,
        ffi::Py_ssize_t,
    ) -> c_int,
    pub(super) compiled: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawFrameView,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
    ) -> c_int,
    pub(super) created: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *const RawFrameView,
        *mut ffi::PyObject,
        c_uint,
    ) -> c_int,
    pub(super) validate_component: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        c_uint,
        ffi::Py_ssize_t,
    ) -> c_int,
    pub(super) prepare_slots: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawFrameView,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut *mut ffi::PyObject,
    ) -> c_int,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::mem::{offset_of, size_of};

    use super::*;

    #[test]
    fn dataclass_callbacks_match_the_selected_native_layout() -> PyResult<()> {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let native: BTreeMap<String, usize> = py
                .import("_testinternalcapi")?
                .call_method0("soac_type_construction_layout")?
                .extract()?;
            for (name, actual) in [
                ("dataclass_abi_version", CALLBACKS_ABI as usize),
                ("dataclass_callbacks_size", size_of::<Callbacks>()),
                ("prepare_slots", offset_of!(Callbacks, prepare_slots)),
            ] {
                assert_eq!(
                    native.get(name),
                    Some(&actual),
                    "selected dataclass ABI: {name}"
                );
            }
            Ok(())
        })
    }
}

unsafe extern "C" {
    pub(super) fn PySoac_SetDataclassCallbacks(callbacks: *const Callbacks) -> c_int;
    pub(super) fn PySoac_NewDataclassInvocation(owner: *mut ffi::PyObject) -> *mut ffi::PyObject;
    pub(super) fn PySoac_DataclassVectorcall(
        invocation: *mut ffi::PyObject,
        stage: c_uint,
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargsf: usize,
        names: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    pub(super) fn PySoac_DataclassBindClass(
        invocation: *mut ffi::PyObject,
        class: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
    ) -> c_int;
    pub(super) fn PySoac_DataclassMatchesSlotsClass(
        invocation: *mut ffi::PyObject,
        class: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
    ) -> c_int;
    pub(super) fn PySoac_CompleteDataclassInvocation(invocation: *mut ffi::PyObject) -> c_int;
    pub(super) fn PySoac_FailDataclassInvocation(invocation: *mut ffi::PyObject) -> c_int;
    pub(super) fn PySoac_DeclineDataclassInvocation(invocation: *mut ffi::PyObject) -> c_int;
    pub(super) fn PySoac_GetDataclassBuiltin(kind: c_uint) -> *mut ffi::PyObject;
    pub(super) fn PyFunction_MatchesSoacDataclassCreation(
        function: *mut ffi::PyObject,
        invocation: *mut ffi::PyObject,
        role: c_uint,
    ) -> c_int;
    pub(super) fn PyFunction_HasSoacDataclassCreation(function: *mut ffi::PyObject) -> c_int;
    pub(super) fn PyFunction_GetSoacStrictId(function: *mut ffi::PyObject) -> u64;
    pub(super) fn PyFunction_AdoptSoacDataclassComponent(
        invocation: *mut ffi::PyObject,
        method: *mut ffi::PyObject,
        component: *mut ffi::PyObject,
        kind: c_uint,
        closure_index: ffi::Py_ssize_t,
    ) -> c_int;
    fn PySoac_DataclassFrameInvocation(view: *const RawFrameView) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameFunction(view: *const RawFrameView) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameCode(view: *const RawFrameView) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameGlobals(view: *const RawFrameView) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameBuiltins(view: *const RawFrameView) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameLocal(
        view: *const RawFrameView,
        index: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameCellValue(
        view: *const RawFrameView,
        index: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
    fn PySoac_DataclassFrameRole(view: *const RawFrameView) -> c_uint;
    fn PySoac_DataclassFrameInstruction(view: *const RawFrameView) -> ffi::Py_ssize_t;
}

/// Callback-lifetime borrowed native view. Never stored in a Python owner or
/// reconstructed by walking current_frame/TLS. No method executes Python or
/// materializes f_locals, an argument tuple, or an instance dictionary.
#[derive(Clone, Copy)]
pub(super) struct Frame<'a> {
    raw: NonNull<RawFrameView>,
    _lifetime: std::marker::PhantomData<&'a RawFrameView>,
}

impl<'a> Frame<'a> {
    /// The caller must keep the native callback's actual view alive for 'a.
    pub(super) unsafe fn from_raw(raw: *const RawFrameView) -> Option<Self> {
        NonNull::new(raw.cast_mut()).map(|raw| Self {
            raw,
            _lifetime: std::marker::PhantomData,
        })
    }

    pub(super) fn as_raw(self) -> *const RawFrameView {
        self.raw.as_ptr()
    }

    pub(super) fn invocation(self) -> *mut ffi::PyObject {
        unsafe { PySoac_DataclassFrameInvocation(self.raw.as_ptr()) }
    }
    pub(super) fn function(self) -> *mut ffi::PyObject {
        unsafe { PySoac_DataclassFrameFunction(self.raw.as_ptr()) }
    }
    pub(super) fn code(self) -> *mut ffi::PyObject {
        unsafe { PySoac_DataclassFrameCode(self.raw.as_ptr()) }
    }
    pub(super) fn globals(self) -> *mut ffi::PyObject {
        unsafe { PySoac_DataclassFrameGlobals(self.raw.as_ptr()) }
    }
    pub(super) fn builtins(self) -> *mut ffi::PyObject {
        unsafe { PySoac_DataclassFrameBuiltins(self.raw.as_ptr()) }
    }
    pub(super) fn role(self) -> c_uint {
        unsafe { PySoac_DataclassFrameRole(self.raw.as_ptr()) }
    }
    pub(super) fn instruction(self) -> Option<usize> {
        usize::try_from(unsafe { PySoac_DataclassFrameInstruction(self.raw.as_ptr()) }).ok()
    }

    pub(super) fn parameter(
        self,
        py: Python<'_>,
        recipe: &CodeRecipe,
        name: &str,
    ) -> PyResult<*mut ffi::PyObject> {
        let index = recipe.parameter_index(name).ok_or_else(|| {
            strict_runtime_unavailable(
                py,
                "dataclass parameter projection is absent from its recipe",
            )
        })?;
        self.local(py, index)
    }

    pub(super) fn local(self, py: Python<'_>, index: usize) -> PyResult<*mut ffi::PyObject> {
        checked_borrow(py, unsafe {
            PySoac_DataclassFrameLocal(self.raw.as_ptr(), index as ffi::Py_ssize_t)
        })
    }

    pub(super) fn binding(
        self,
        py: Python<'_>,
        binding: FrameBinding,
    ) -> PyResult<*mut ffi::PyObject> {
        match binding {
            FrameBinding::Local(index) => self.local(py, index),
            FrameBinding::Cell(index) => checked_borrow(py, unsafe {
                PySoac_DataclassFrameCellValue(self.raw.as_ptr(), index as ffi::Py_ssize_t)
            }),
        }
    }

    pub(super) fn executing(
        self,
        py: Python<'_>,
        recipe: &CodeRecipe,
        name: &str,
    ) -> PyResult<*mut ffi::PyObject> {
        let binding = recipe.executing_binding(name).ok_or_else(|| {
            strict_runtime_unavailable(py, "dataclass executing binding is absent from its recipe")
        })?;
        self.binding(py, binding)
    }
}

fn checked_borrow(py: Python<'_>, value: *mut ffi::PyObject) -> PyResult<*mut ffi::PyObject> {
    if value.is_null() && unsafe { !ffi::PyErr_Occurred().is_null() } {
        Err(PyErr::fetch(py))
    } else {
        Ok(value) // NULL/unbound is not Python None and confers no value proof.
    }
}

pub(super) fn status(py: Python<'_>, value: c_int) -> PyResult<()> {
    if value < 0 {
        Err(PyErr::fetch(py))
    } else {
        Ok(())
    }
}

pub(super) fn predicate(py: Python<'_>, value: c_int) -> PyResult<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(PyErr::fetch(py)),
    }
}
