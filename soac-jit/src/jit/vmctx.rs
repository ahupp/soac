use super::ObjPtr;
use crate::module_globals::ModuleGlobalCache;
use crate::module_type::SharedModuleState;
use pyo3::ffi;
use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr;
use std::sync::Arc;

#[repr(C)]
pub struct JitModuleVmCtx {
    pub shared_module_state: *const SharedModuleState,
    pub globals_obj: ObjPtr,
    pub global_slots: ObjPtr,
    pub true_obj: ObjPtr,
    pub false_obj: ObjPtr,
    pub none_obj: ObjPtr,
    pub deleted_obj: ObjPtr,
    pub empty_tuple_obj: ObjPtr,
}

pub struct ModuleRuntimeContext {
    pub vmctx: JitModuleVmCtx,
    pub shared_module_state_owner: Arc<SharedModuleState>,
    pub global_cache_owner: Arc<ModuleGlobalCache>,
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
        self.vmctx.globals_obj = ptr::null_mut::<c_void>();
        self.vmctx.global_slots = ptr::null_mut::<c_void>();
        self.vmctx.true_obj = ptr::null_mut::<c_void>();
        self.vmctx.false_obj = ptr::null_mut::<c_void>();
        self.vmctx.none_obj = ptr::null_mut::<c_void>();
        self.vmctx.deleted_obj = ptr::null_mut::<c_void>();
        self.vmctx.empty_tuple_obj = ptr::null_mut::<c_void>();
    }
}

pub const GLOBALS_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, globals_obj) as i32;
pub const GLOBAL_SLOTS_OFFSET: i32 = offset_of!(JitModuleVmCtx, global_slots) as i32;
pub const TRUE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, true_obj) as i32;
pub const FALSE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, false_obj) as i32;
pub const NONE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, none_obj) as i32;
pub const DELETED_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, deleted_obj) as i32;
pub const EMPTY_TUPLE_OBJ_OFFSET: i32 = offset_of!(JitModuleVmCtx, empty_tuple_obj) as i32;
