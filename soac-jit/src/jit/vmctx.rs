use super::ObjPtr;
use crate::module_type::SharedModuleState;
use pyo3::ffi;
use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr;
use std::sync::Arc;

#[repr(C)]
struct PyThreadStateCurrentExceptionPrefix {
    prev: *mut ffi::PyThreadState,
    next: *mut ffi::PyThreadState,
    interp: *mut ffi::PyInterpreterState,
    eval_breaker: usize,
    status: u32,
    holds_gil: i32,
    gil_requested: i32,
    whence: i32,
    state: i32,
    py_recursion_remaining: i32,
    py_recursion_limit: i32,
    recursion_headroom: i32,
    tracing: i32,
    what_event: i32,
    current_frame: *mut c_void,
    base_frame: *mut c_void,
    last_profiled_frame: *mut c_void,
    c_profilefunc: *mut c_void,
    c_tracefunc: *mut c_void,
    c_profileobj: *mut ffi::PyObject,
    c_traceobj: *mut ffi::PyObject,
    current_exception: *mut ffi::PyObject,
}

#[repr(C)]
pub struct JitModuleVmCtx {
    pub shared_module_state: *const SharedModuleState,
    pub current_exception_slot: ObjPtr,
    pub globals_obj: ObjPtr,
    pub true_obj: ObjPtr,
    pub false_obj: ObjPtr,
    pub none_obj: ObjPtr,
    pub deleted_obj: ObjPtr,
    pub empty_tuple_obj: ObjPtr,
}

pub struct ModuleRuntimeContext {
    pub vmctx: JitModuleVmCtx,
    pub shared_module_state_owner: Arc<SharedModuleState>,
}

pub unsafe fn current_thread_raised_exception_slot() -> ObjPtr {
    let tstate = unsafe { ffi::PyThreadState_Get() };
    unsafe {
        ptr::addr_of_mut!((*(tstate as *mut PyThreadStateCurrentExceptionPrefix)).current_exception)
            .cast::<c_void>()
    }
}

impl JitModuleVmCtx {
    pub unsafe fn refresh_current_exception_slot(&mut self) {
        self.current_exception_slot = unsafe { current_thread_raised_exception_slot() };
    }
}

unsafe fn decref_if_non_null(obj: ObjPtr) {
    if !obj.is_null() {
        unsafe { ffi::Py_DECREF(obj.cast::<ffi::PyObject>()) };
    }
}

impl Drop for ModuleRuntimeContext {
    fn drop(&mut self) {
        unsafe {
            decref_if_non_null(self.vmctx.globals_obj);
            decref_if_non_null(self.vmctx.true_obj);
            decref_if_non_null(self.vmctx.false_obj);
            decref_if_non_null(self.vmctx.none_obj);
            decref_if_non_null(self.vmctx.deleted_obj);
            decref_if_non_null(self.vmctx.empty_tuple_obj);
        }
        self.vmctx.shared_module_state = ptr::null();
        self.vmctx.current_exception_slot = ptr::null_mut::<c_void>();
        self.vmctx.globals_obj = ptr::null_mut::<c_void>();
        self.vmctx.true_obj = ptr::null_mut::<c_void>();
        self.vmctx.false_obj = ptr::null_mut::<c_void>();
        self.vmctx.none_obj = ptr::null_mut::<c_void>();
        self.vmctx.deleted_obj = ptr::null_mut::<c_void>();
        self.vmctx.empty_tuple_obj = ptr::null_mut::<c_void>();
    }
}

pub const CURRENT_EXCEPTION_SLOT_OFFSET: i32 =
    offset_of!(JitModuleVmCtx, current_exception_slot) as i32;
pub const GLOBALS_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, globals_obj) as i32;
pub const TRUE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, true_obj) as i32;
pub const FALSE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, false_obj) as i32;
pub const NONE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, none_obj) as i32;
pub const DELETED_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, deleted_obj) as i32;
pub const EMPTY_TUPLE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, empty_tuple_obj) as i32;
