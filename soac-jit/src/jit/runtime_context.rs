use super::ObjPtr;
use crate::module_type::SharedModuleState;
use crate::session::CompileSession;
use pyo3::ffi;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
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
pub struct ModuleJitContext {
    pub shared_module_state: *const SharedModuleState,
    pub globals_obj: ObjPtr,
}

#[repr(C)]
struct FunctionEnvPrefix {
    direct_code_ptr: *const u8,
    default_direct_code_ptr: *const u8,
    globals_obj: ObjPtr,
}

#[repr(C)]
struct PyFunctionJitExtraPrefix {
    function_env: ObjPtr,
    function_id: u64,
}

pub struct ModuleRuntimeContext {
    pub mod_ctx: ModuleJitContext,
    pub compile_session: Arc<CompileSession>,
    pub shared_module_state_owner: Arc<SharedModuleState>,
}

unsafe fn decref_if_non_null(obj: ObjPtr) {
    if !obj.is_null() {
        unsafe { ffi::Py_DECREF(obj.cast::<ffi::PyObject>()) };
    }
}

impl Drop for ModuleRuntimeContext {
    fn drop(&mut self) {
        unsafe {
            decref_if_non_null(self.mod_ctx.globals_obj);
        }
        self.mod_ctx.shared_module_state = ptr::null();
        self.mod_ctx.globals_obj = ptr::null_mut::<c_void>();
    }
}

pub const FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET: i32 =
    offset_of!(FunctionEnvPrefix, direct_code_ptr) as i32;
pub const FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET: i32 =
    offset_of!(FunctionEnvPrefix, default_direct_code_ptr) as i32;
pub const FUNCTION_ENV_GLOBALS_OBJ_OFFSET: i32 = offset_of!(FunctionEnvPrefix, globals_obj) as i32;
pub const FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET: i32 = size_of::<FunctionEnvPrefix>() as i32;
pub const PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET: i32 =
    offset_of!(PyFunctionJitExtraPrefix, function_env) as i32;
pub const PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET: i32 =
    offset_of!(PyThreadStateCurrentExceptionPrefix, current_exception) as i32;
