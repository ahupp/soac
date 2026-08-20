//! The versioned CPython interpreter callback ABI. No private frame layout is
//! mirrored here: every value is borrowed through a callback-scoped C view.

use std::ffi::c_int;
use std::mem::{MaybeUninit, size_of};

use pyo3::ffi;
use pyo3::prelude::*;

pub(super) const ROOT: u32 = 1;
pub(super) const FUNCTION: u32 = 2;
pub(super) const CLASS_NAMESPACE: u32 = 3;
pub(super) const BINDING: u32 = 1;
pub(super) const RUNNING: u32 = 3;
pub(super) const RETURNING: u32 = 4;
pub(super) const FAILING: u32 = 6;
pub(super) const NAMESPACE_TRANSFERRED: u32 = 6;

#[repr(C)]
pub(super) struct RawInterpreterFrameView {
    _opaque: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RawInterpreterFrameInfo {
    pub(super) abi_version: u32,
    pub(super) phase: u32,
    pub(super) function: *mut ffi::PyObject,
    pub(super) code: *mut ffi::PyObject,
    pub(super) globals: *mut ffi::PyObject,
    pub(super) builtins: *mut ffi::PyObject,
    pub(super) locals: *mut ffi::PyObject,
    pub(super) call_state: *mut ffi::PyObject,
    pub(super) instruction_units: ffi::Py_ssize_t,
    pub(super) instruction_ordinal: ffi::Py_ssize_t,
    pub(super) localsplus_count: ffi::Py_ssize_t,
}

pub(super) const CALL_SELECT: u32 = 1;
pub(super) const CALL_PREPARE_TYPE: u32 = 2;
pub(super) const CALL_VECTOR: u32 = 1;
pub(super) const CALL_VECTOR_KW: u32 = 2;
pub(super) const CALL_EXPANDED: u32 = 3;
pub(super) const CALL_NULL_CHANNEL: u32 = 0;
pub(super) const CALL_VALUE_CHANNEL: u32 = 1;
pub(super) const DECORATORS_NONE: u32 = 0;
pub(super) const DECORATORS_CURRENT: u32 = 1;
pub(super) const DECORATORS_DIRECT_CALLER: u32 = 2;
pub(super) const OPERAND_CALLABLE: u32 = 1;
pub(super) const OPERAND_POSITIONAL: u32 = 2;
pub(super) const OPERAND_KEYWORD_VALUE: u32 = 3;
pub(super) const OPERAND_KEYWORD_NAMES: u32 = 4;
pub(super) const OPERAND_EXPANDED_ARGS: u32 = 5;
pub(super) const OPERAND_EXPANDED_KWARGS: u32 = 6;
pub(super) const OPERAND_DECORATOR: u32 = 7;
pub(super) const CREATION_NONE: u32 = 0;
pub(super) const CREATION_LIVE: u32 = 1;
pub(super) const CREATION_INVALID: u32 = 2;
pub(super) const CALL_ORDINARY: u32 = 0;
pub(super) const CALL_DATACLASS_ROOT: u32 = 1;
pub(super) const CALL_BUILTIN_DESCRIPTOR: u32 = 2;
pub(super) const CALL_CLASS: u32 = 3;
pub(super) const CALL_GENERIC_SCOPE: u32 = 4;

#[repr(C)]
pub(super) struct RawInterpreterCallView {
    _opaque: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RawInterpreterCallSite {
    pub(super) form: u32,
    pub(super) channel: u32,
    pub(super) instruction_argument: u32,
    pub(super) reserved: u32,
    pub(super) positional_count: ffi::Py_ssize_t,
    pub(super) keyword_count: ffi::Py_ssize_t,
    pub(super) frame: *const RawInterpreterFrameView,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RawInterpreterCallInfo {
    pub(super) abi_version: u32,
    pub(super) phase: u32,
    pub(super) current: RawInterpreterCallSite,
    pub(super) direct_caller: *const RawInterpreterCallSite,
    pub(super) decorator_source: u32,
    pub(super) reserved: u32,
    pub(super) decorator_count: ffi::Py_ssize_t,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RawInterpreterCallOperand {
    pub(super) abi_version: u32,
    pub(super) creation_status: u32,
    pub(super) creation_role: u32,
    pub(super) reserved: u32,
    pub(super) creation_identity: u64,
    pub(super) value: *mut ffi::PyObject,
    pub(super) dataclass_invocation: *mut ffi::PyObject,
    pub(super) dataclass_owner: *mut ffi::PyObject,
}

#[repr(C)]
pub(super) struct RawInterpreterCallDecision {
    pub(super) abi_version: u32,
    pub(super) kind: u32,
    pub(super) dataclass_stage: u32,
    pub(super) decorator_source: u32,
    pub(super) decorator_count: ffi::Py_ssize_t,
    pub(super) metadata: *mut ffi::PyObject,
    pub(super) expected_function_owner: *mut ffi::PyObject,
    pub(super) verified_code: *mut ffi::PyObject,
}

impl RawInterpreterCallDecision {
    pub(super) const fn ordinary() -> Self {
        Self {
            abi_version: 1,
            kind: CALL_ORDINARY,
            dataclass_stage: 0,
            decorator_source: DECORATORS_NONE,
            decorator_count: 0,
            metadata: std::ptr::null_mut(),
            expected_function_owner: std::ptr::null_mut(),
            verified_code: std::ptr::null_mut(),
        }
    }
}

#[repr(C)]
struct RawInterpreterCallbacks {
    abi_version: u32,
    flags: u32,
    root_begin:
        unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject, *mut ffi::PyObject) -> c_int,
    root_end: unsafe extern "C" fn(*mut ffi::PyObject, c_int),
    birth: unsafe extern "C" fn(
        *const RawInterpreterFrameView,
        *mut ffi::PyObject,
        *mut *mut ffi::PyObject,
    ) -> c_int,
    function_attribute: unsafe extern "C" fn(
        *const RawInterpreterFrameView,
        *mut ffi::PyObject,
        u32,
        *mut ffi::PyObject,
    ) -> c_int,
    enter: unsafe extern "C" fn(
        u32,
        *mut ffi::PyObject,
        *const RawInterpreterFrameView,
        *const RawInterpreterFrameView,
        *mut *mut ffi::PyObject,
    ) -> c_int,
    started: unsafe extern "C" fn(*mut ffi::PyObject, *const RawInterpreterFrameView) -> c_int,
    call: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawInterpreterCallInfo,
        *const RawInterpreterCallView,
        *mut RawInterpreterCallDecision,
        usize,
    ) -> c_int,
    selected_call_finished: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawInterpreterFrameView,
        u32,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        u32,
        *mut ffi::PyObject,
    ) -> c_int,
    returned: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawInterpreterFrameView,
        *mut ffi::PyObject,
    ) -> c_int,
    failed: unsafe extern "C" fn(*mut ffi::PyObject, *const RawInterpreterFrameView) -> c_int,
    leave: unsafe extern "C" fn(*mut ffi::PyObject, u32),
    prepare_type: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *const RawInterpreterFrameView,
        *const RawInterpreterCallInfo,
        *const RawInterpreterCallView,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut *mut ffi::PyObject,
    ) -> c_int,
    definition_store:
        unsafe extern "C" fn(*const RawInterpreterFrameView, u32, *mut ffi::PyObject) -> c_int,
}

unsafe extern "C" {
    fn PySoac_SetInterpreterCallbacksV2(
        callbacks: *const RawInterpreterCallbacks,
        size: usize,
    ) -> c_int;
    pub(super) fn PySoac_EvalInterpreterModuleV1(
        module: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_GetInterpreterFrameInfoV1(
        view: *const RawInterpreterFrameView,
        out: *mut RawInterpreterFrameInfo,
        size: usize,
    ) -> c_int;
    fn PySoac_InterpreterCallOperandV1(
        view: *const RawInterpreterCallView,
        kind: u32,
        index: ffi::Py_ssize_t,
        out: *mut RawInterpreterCallOperand,
        size: usize,
    ) -> c_int;
}

const CALLBACKS: RawInterpreterCallbacks = RawInterpreterCallbacks {
    abi_version: 2,
    flags: 0,
    root_begin: super::callbacks::root_begin,
    root_end: super::callbacks::root_end,
    birth: super::callbacks::birth,
    function_attribute: super::callbacks::function_attribute,
    enter: super::callbacks::enter,
    started: super::callbacks::started,
    call: super::call_join::select,
    selected_call_finished: super::call_join::selected_call_finished,
    returned: super::callbacks::returned,
    failed: super::callbacks::failed,
    leave: super::callbacks::leave,
    prepare_type: super::callbacks::prepare_type,
    definition_store: super::callbacks::definition_store,
};

pub(super) fn initialize(py: Python<'_>) -> PyResult<()> {
    if unsafe { PySoac_SetInterpreterCallbacksV2(&CALLBACKS, size_of::<RawInterpreterCallbacks>()) }
        < 0
    {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}

/// # Safety
/// The view is supplied by the active callback and remains supported until it
/// returns. None of these borrowed pointers may escape that interval.
pub(super) unsafe fn frame_info(
    py: Python<'_>,
    view: *const RawInterpreterFrameView,
) -> PyResult<RawInterpreterFrameInfo> {
    let mut result = MaybeUninit::uninit();
    if unsafe {
        PySoac_GetInterpreterFrameInfoV1(
            view,
            result.as_mut_ptr(),
            size_of::<RawInterpreterFrameInfo>(),
        )
    } < 0
    {
        return Err(PyErr::fetch(py));
    }
    Ok(unsafe { result.assume_init() })
}

/// # Safety
/// The native callback supports this opaque view and every returned pointer.
/// The result cannot outlive that callback; no Python reference is acquired.
pub(super) unsafe fn call_operand(
    py: Python<'_>,
    view: *const RawInterpreterCallView,
    kind: u32,
    index: usize,
) -> PyResult<RawInterpreterCallOperand> {
    let index = ffi::Py_ssize_t::try_from(index).map_err(|_| {
        crate::strict_runtime_unavailable(py, "native call operand index is out of range")
    })?;
    let mut out = MaybeUninit::uninit();
    if unsafe {
        PySoac_InterpreterCallOperandV1(
            view,
            kind,
            index,
            out.as_mut_ptr(),
            size_of::<RawInterpreterCallOperand>(),
        )
    } < 0
    {
        return Err(PyErr::fetch(py));
    }
    let out = unsafe { out.assume_init() };
    let valid_creation = match out.creation_status {
        CREATION_NONE | CREATION_INVALID => {
            out.creation_role == 0
                && out.creation_identity == 0
                && out.dataclass_invocation.is_null()
                && out.dataclass_owner.is_null()
        }
        CREATION_LIVE => {
            out.creation_role != 0
                && out.creation_identity != 0
                && !out.dataclass_invocation.is_null()
                && !out.dataclass_owner.is_null()
        }
        _ => false,
    };
    if out.abi_version != 1 || out.reserved != 0 || !valid_creation {
        return Err(crate::strict_runtime_unavailable(
            py,
            "malformed native call operand view",
        ));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) const INTERPRETER_ABI_LAYOUT: [(&str, usize); 8] = [
    ("abi_version", 1),
    ("callbacks_size", size_of::<RawInterpreterCallbacks>()),
    ("frame_info_size", size_of::<RawInterpreterFrameInfo>()),
    (
        "instruction_ordinal",
        std::mem::offset_of!(RawInterpreterFrameInfo, instruction_ordinal),
    ),
    ("call_site_size", size_of::<RawInterpreterCallSite>()),
    ("call_info_size", size_of::<RawInterpreterCallInfo>()),
    ("call_operand_size", size_of::<RawInterpreterCallOperand>()),
    (
        "call_decision_size",
        size_of::<RawInterpreterCallDecision>(),
    ),
];
