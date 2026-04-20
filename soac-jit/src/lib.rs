#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_unsafe)]

include!(concat!(env!("OUT_DIR"), "/soac_runtime_clif.rs"));

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

pub mod config;
pub mod function_instantiation;
pub mod import_helpers;
mod jit;
pub(crate) mod operator_specialization;
pub use function_instantiation::{
    instantiate_bb_function, make_function, make_function_from_python_args,
};
pub use jit::*;

pub mod counter;
pub mod module_constants;
pub mod module_type;
pub mod optimization_alternatives_v3;
pub mod optimization_plan;
pub mod optimization_plan_v3;
pub mod session;

pub use session::{CompileSession, CompileSessionId, allocate_compile_session_id};

#[cfg(test)]
pub(crate) fn python_runtime_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

use pyo3::ffi;
use pyo3::prelude::*;
use soac_core::block_py::{FunctionExecutionMode, FunctionKind, ParamKind, RuntimeFunctionId};
use soac_lowering::passes::CodegenModuleShape;
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::any::Any;
use std::ffi::{CString, c_char, c_void};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tracing::info;

#[cfg(test)]
pub(crate) fn test_repo_root() -> std::path::PathBuf {
    soac_cpython::repo_root()
}

#[cfg(test)]
pub(crate) fn initialize_test_python() {
    soac_cpython::initialize_test_python("soac-jit-test").expect("test Python should initialize");
}

#[cfg(test)]
pub(crate) fn run_test_in_isolated_process_if_needed(module_path: &str, test_name: &str) -> bool {
    let marker_key = format!("SOAC_TEST_ISOLATED_{test_name}");
    if std::env::var_os(&marker_key).is_some() {
        return false;
    }

    let test_module = module_path
        .split_once("::")
        .map(|(_, rest)| rest)
        .unwrap_or(module_path);
    let full_test_name = format!("{test_module}::{test_name}");
    let current_exe =
        std::env::current_exe().expect("isolated test runner should locate current test binary");
    let status = std::process::Command::new(current_exe)
        .arg(&full_test_name)
        .arg("--exact")
        .arg("--test-threads=1")
        .env(&marker_key, "1")
        .status()
        .unwrap_or_else(|err| {
            panic!("isolated test runner should spawn child for {full_test_name}: {err}")
        });
    assert!(
        status.success(),
        "isolated test process failed for {full_test_name} with status {status}"
    );
    true
}

unsafe extern "C" {
    fn PyFunction_SetVectorcall(
        func: *mut ffi::PyFunctionObject,
        vectorcall: Option<ffi::vectorcallfunc>,
    );
    fn PyFunction_SetSoacMetadata(
        function: *mut ffi::PyObject,
        soac_function_id: u64,
        metadata: *mut c_void,
        destructor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> i32;
    fn PyFunction_GetSoacMetadata(function: *mut ffi::PyObject) -> *mut c_void;
    fn PyFunction_GetSoacFunctionId(function: *mut ffi::PyObject) -> u64;
    fn PyFunction_GetDefaults(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyFunction_GetKwDefaults(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyFunction_GetClosure(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyType_SetSoacMetadata(
        type_obj: *mut ffi::PyObject,
        soac_function_id: u64,
        metadata: *mut c_void,
        destructor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> i32;
    #[cfg(test)]
    fn PyType_GetSoacMetadata(type_obj: *mut ffi::PyObject) -> *mut c_void;
    fn PyType_GetSoacFunctionId(type_obj: *mut ffi::PyObject) -> u64;
    fn PyFunction_AddWatcher(callback: PyFunctionWatchCallback) -> i32;
    fn PyType_Modified(type_obj: *mut ffi::PyTypeObject);
    fn PyUnstable_Type_AssignVersionTag(type_obj: *mut ffi::PyTypeObject) -> i32;
    static mut PyType_Type: ffi::PyTypeObject;
    static mut PyBaseObject_Type: ffi::PyTypeObject;
    fn PyType_GenericAlloc(
        type_obj: *mut ffi::PyTypeObject,
        nitems: ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject;
    fn PyWeakref_NewRef(
        object: *mut ffi::PyObject,
        callback: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PyWeakref_GetRef(reference: *mut ffi::PyObject, object: *mut *mut ffi::PyObject) -> i32;
    fn _PyDict_IndexedKeyIndex(
        dict: *mut ffi::PyObject,
        key: *mut ffi::PyObject,
    ) -> ffi::Py_ssize_t;
    fn _PyDict_GetIndexedItem(
        dict: *mut ffi::PyObject,
        index: ffi::Py_ssize_t,
        result: *mut *mut ffi::PyObject,
    ) -> i32;
}

type PyFunctionWatchEvent = i32;
type PyFunctionWatchCallback = unsafe extern "C" fn(
    event: PyFunctionWatchEvent,
    func: *mut ffi::PyFunctionObject,
    new_value: *mut ffi::PyObject,
) -> i32;

const PY_FUNCTION_EVENT_CREATE: PyFunctionWatchEvent = 0;
const PY_FUNCTION_EVENT_DESTROY: PyFunctionWatchEvent = 1;
const PY_FUNCTION_EVENT_MODIFY_CODE: PyFunctionWatchEvent = 2;
const PY_FUNCTION_EVENT_MODIFY_DEFAULTS: PyFunctionWatchEvent = 3;
const PY_FUNCTION_EVENT_MODIFY_KWDEFAULTS: PyFunctionWatchEvent = 4;
const PY_FUNCTION_EVENT_MODIFY_QUALNAME: PyFunctionWatchEvent = 5;

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

unsafe extern "C" fn function_owner_type_watcher_callback(
    event: PyFunctionWatchEvent,
    func: *mut ffi::PyFunctionObject,
    new_value: *mut ffi::PyObject,
) -> i32 {
    let Some(Ok(registry)) = FUNCTION_OWNER_TYPE_REGISTRY.get() else {
        return 0;
    };

    match event {
        PY_FUNCTION_EVENT_MODIFY_CODE
        | PY_FUNCTION_EVENT_MODIFY_DEFAULTS
        | PY_FUNCTION_EVENT_MODIFY_KWDEFAULTS
        | PY_FUNCTION_EVENT_MODIFY_QUALNAME => {
            if event == PY_FUNCTION_EVENT_MODIFY_CODE {
                let metadata = unsafe { PyFunction_GetSoacMetadata(func as *mut ffi::PyObject) };
                if !metadata.is_null() {
                    let data = unsafe { &mut *(metadata as *mut PyFunctionJitExtra) };
                    unsafe { PyFunction_SetVectorcall(func, data.previous_vectorcall) };
                }
            }
            if event == PY_FUNCTION_EVENT_MODIFY_DEFAULTS
                || event == PY_FUNCTION_EVENT_MODIFY_KWDEFAULTS
            {
                let metadata = unsafe { PyFunction_GetSoacMetadata(func as *mut ffi::PyObject) };
                if !metadata.is_null() {
                    let data = unsafe { &mut *(metadata as *mut PyFunctionJitExtra) };
                    if unsafe {
                        data.refresh_runtime_objects_after_function_update(
                            func as *mut ffi::PyObject,
                            event,
                            new_value,
                        )
                    }
                    .is_err()
                    {
                        return -1;
                    }
                }
            }
            let weakrefs = match registry.registered_owner_types_by_function.lock() {
                Ok(owner_types_by_function) => owner_types_by_function
                    .get(&(func as usize))
                    .map(|entry| entry.owner_type_weakrefs.clone())
                    .unwrap_or_default(),
                Err(_) => {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"function owner type registry lock poisoned".as_ptr(),
                    );
                    return -1;
                }
            };
            let mut stale_weakrefs = Vec::new();
            for weakref in weakrefs {
                let mut owner_type = ptr::null_mut();
                match PyWeakref_GetRef(weakref as *mut ffi::PyObject, &mut owner_type) {
                    1 => {
                        PyType_Modified(owner_type as *mut ffi::PyTypeObject);
                        ffi::Py_DECREF(owner_type);
                    }
                    0 => stale_weakrefs.push(weakref),
                    _ => return -1,
                }
            }
            if !stale_weakrefs.is_empty() {
                let _ = registry.registered_owner_types_by_function.lock().map(
                    |mut weakrefs_by_function| {
                        if let Some(registered) = weakrefs_by_function.get_mut(&(func as usize)) {
                            registered.owner_type_weakrefs.retain(|weakref| {
                                let keep = !stale_weakrefs.contains(weakref);
                                if !keep {
                                    unsafe { ffi::Py_DECREF(*weakref as *mut ffi::PyObject) };
                                }
                                keep
                            });
                        }
                    },
                );
            }
        }
        PY_FUNCTION_EVENT_DESTROY => {
            let registered = match registry.registered_owner_types_by_function.lock() {
                Ok(mut owner_types_by_function) => owner_types_by_function
                    .remove(&(func as usize))
                    .unwrap_or_else(|| RegisteredFunctionOwnerTypes {
                        function_weakref: 0,
                        owner_type_weakrefs: Vec::new(),
                    }),
                Err(_) => {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"function owner type registry lock poisoned".as_ptr(),
                    );
                    return -1;
                }
            };
            if registered.function_weakref != 0 {
                ffi::Py_DECREF(registered.function_weakref as *mut ffi::PyObject);
            }
            for owner_type in registered.owner_type_weakrefs {
                ffi::Py_DECREF(owner_type as *mut ffi::PyObject);
            }
        }
        PY_FUNCTION_EVENT_CREATE => {}
        _ => {}
    }

    0
}

fn function_owner_type_registry() -> Result<&'static FunctionOwnerTypeRegistry, ()> {
    match FUNCTION_OWNER_TYPE_REGISTRY.get_or_init(|| unsafe {
        let watcher_id = PyFunction_AddWatcher(function_owner_type_watcher_callback);
        if watcher_id < 0 {
            Err(())
        } else {
            Ok(FunctionOwnerTypeRegistry {
                watcher_id,
                registered_owner_types_by_function: Mutex::new(HashMap::new()),
            })
        }
    }) {
        Ok(registry) => Ok(registry),
        Err(()) => Err(()),
    }
}

fn set_runtime_error<T>(msg: &str) -> Result<T, ()> {
    unsafe {
        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, CString::new(msg).unwrap().as_ptr());
    }
    Err(())
}

#[repr(C)]
struct FunctionEnvAbiHeader {
    direct_code_ptr: *const u8,
    default_direct_code_ptr: *const u8,
    deopt_table_ptr: *const c_void,
    globals_obj: *mut ffi::PyObject,
}

struct FunctionEnv {
    abi: NonNull<FunctionEnvAbiHeader>,
    runtime_object_len: usize,
    compiled_function: Option<Arc<jit::CompiledFunctionHandle>>,
}

#[repr(C)]
struct PyFunctionJitExtra {
    function_env_ptr: *mut c_void,
    function_id: RuntimeFunctionId,
    function_env: Box<FunctionEnv>,
    binding_plan: DirectArgBindingPlan,
    entry_plan: jit::RuntimeFunctionEntryPlan,
    compile_session: Arc<CompileSession>,
    module_state: Arc<module_type::SharedModuleState>,
    compiled_vectorcall_entry: Option<jit::VectorcallEntryFn>,
    previous_vectorcall: Option<ffi::vectorcallfunc>,
}

static FORCE_ENTRY_INTERPRETER_VECTORCALL_FOR_TESTS: AtomicBool = AtomicBool::new(false);

struct DirectArgParamBinding {
    name: String,
    kind: ParamKind,
    default_slot: Option<usize>,
}

struct DirectArgBindingPlan {
    callable_name: String,
    params: Box<[DirectArgParamBinding]>,
    positional_param_indices: Box<[usize]>,
    param_indices_by_name: HashMap<String, usize>,
    varargs_param: Option<usize>,
    varkw_param: Option<usize>,
}

impl DirectArgBindingPlan {
    fn from_function(function: &soac_core::block_py::BlockPyFunction<CodegenModuleShape>) -> Self {
        let runtime_data_layout = jit::FunctionRuntimeDataLayout::from_function(function);
        let positional_param_indices = function
            .params
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                matches!(param.kind, ParamKind::PosOnly | ParamKind::Any).then_some(index)
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let varargs_param = function.params.vararg_index();
        let varkw_param = function.params.kwarg_index();
        let mut param_indices_by_name = HashMap::with_capacity(function.params.len());
        let params = function
            .params
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                param_indices_by_name.insert(param.name.clone(), index);
                // Runtime slots exist for all default-capable parameters because
                // __defaults__ / __kwdefaults__ can be assigned after function creation.
                let default_slot = match param.kind {
                    ParamKind::PosOnly | ParamKind::Any => {
                        runtime_data_layout.positional_default_slot_for_param_index(index)
                    }
                    ParamKind::KwOnly => runtime_data_layout.kwonly_default_slot(&param.name),
                    ParamKind::VarArg | ParamKind::KwArg => None,
                };
                DirectArgParamBinding {
                    name: param.name.clone(),
                    kind: param.kind,
                    default_slot,
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            callable_name: function.names.display_name.clone(),
            params,
            positional_param_indices,
            param_indices_by_name,
            varargs_param,
            varkw_param,
        }
    }

    fn param_count(&self) -> usize {
        self.params.len()
    }

    fn positional_capacity(&self) -> usize {
        self.positional_param_indices.len()
    }

    fn param_index(&self, name: &str) -> Option<usize> {
        self.param_indices_by_name.get(name).copied()
    }
}

impl FunctionEnv {
    fn runtime_objects_offset() -> usize {
        mem::size_of::<FunctionEnvAbiHeader>()
    }

    fn allocation_layout(runtime_object_len: usize) -> Layout {
        let header_size = mem::size_of::<FunctionEnvAbiHeader>();
        let runtime_object_size = runtime_object_len
            .checked_mul(mem::size_of::<*mut ffi::PyObject>())
            .expect("function runtime object block is too large");
        let size = header_size
            .checked_add(runtime_object_size)
            .expect("function env allocation is too large");
        Layout::from_size_align(size.max(1), mem::align_of::<FunctionEnvAbiHeader>())
            .expect("function env allocation layout should be valid")
    }

    unsafe fn new(
        globals_obj: *mut ffi::PyObject,
        mut runtime_object_values: Box<[*mut ffi::PyObject]>,
    ) -> Result<Self, ()> {
        if globals_obj.is_null() {
            unsafe { cleanup_state_values(&mut runtime_object_values) };
            return set_runtime_error("missing globals while creating JIT function environment");
        }
        unsafe { ffi::Py_INCREF(globals_obj) };
        let runtime_object_len = runtime_object_values.len();
        let layout = Self::allocation_layout(runtime_object_len);
        let raw = unsafe { alloc(layout) };
        let Some(abi) = NonNull::new(raw as *mut FunctionEnvAbiHeader) else {
            handle_alloc_error(layout);
        };
        unsafe {
            abi.as_ptr().write(FunctionEnvAbiHeader {
                direct_code_ptr: ptr::null(),
                default_direct_code_ptr: ptr::null(),
                deopt_table_ptr: ptr::null(),
                globals_obj,
            });
            let runtime_objects =
                raw.add(Self::runtime_objects_offset()) as *mut *mut ffi::PyObject;
            ptr::copy_nonoverlapping(
                runtime_object_values.as_ptr(),
                runtime_objects,
                runtime_object_len,
            );
        }
        runtime_object_values.fill(ptr::null_mut());
        Ok(Self {
            abi,
            runtime_object_len,
            compiled_function: None,
        })
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.abi.as_ptr() as *mut c_void
    }

    fn header(&self) -> &FunctionEnvAbiHeader {
        unsafe { self.abi.as_ref() }
    }

    fn header_mut(&mut self) -> &mut FunctionEnvAbiHeader {
        unsafe { self.abi.as_mut() }
    }

    fn globals_obj(&self) -> *mut ffi::PyObject {
        self.header().globals_obj
    }

    fn direct_code_ptr(&self) -> *const u8 {
        self.header().direct_code_ptr
    }

    fn set_direct_code_ptr(&mut self, direct_code_ptr: *const u8) {
        self.header_mut().direct_code_ptr = direct_code_ptr;
    }

    fn default_direct_code_ptr(&self) -> *const u8 {
        self.header().default_direct_code_ptr
    }

    fn set_default_direct_code_ptr(&mut self, default_direct_code_ptr: *const u8) {
        self.header_mut().default_direct_code_ptr = default_direct_code_ptr;
    }

    fn deopt_table_ptr(&self) -> *const c_void {
        self.header().deopt_table_ptr
    }

    fn set_deopt_table_ptr(&mut self, deopt_table_ptr: *const c_void) {
        self.header_mut().deopt_table_ptr = deopt_table_ptr;
    }

    fn runtime_objects_mut(&mut self) -> &mut [*mut ffi::PyObject] {
        unsafe {
            let base = self.abi.as_ptr() as *mut u8;
            let runtime_objects =
                base.add(Self::runtime_objects_offset()) as *mut *mut ffi::PyObject;
            std::slice::from_raw_parts_mut(runtime_objects, self.runtime_object_len)
        }
    }

    fn runtime_objects_ptr(&self) -> *mut ffi::PyObject {
        unsafe {
            let base = self.abi.as_ptr() as *mut u8;
            base.add(Self::runtime_objects_offset()) as *mut ffi::PyObject
        }
    }

    fn runtime_object(&self, slot: usize) -> *mut ffi::PyObject {
        if slot >= self.runtime_object_len {
            return ptr::null_mut();
        }
        unsafe {
            let base = self.abi.as_ptr() as *mut u8;
            let runtime_objects =
                base.add(Self::runtime_objects_offset()) as *mut *mut ffi::PyObject;
            *runtime_objects.add(slot)
        }
    }

    unsafe fn replace_runtime_objects(
        &mut self,
        mut new_values: Box<[*mut ffi::PyObject]>,
    ) -> Result<(), ()> {
        if new_values.len() != self.runtime_object_len {
            unsafe { cleanup_state_values(&mut new_values) };
            return Err(());
        }
        for (slot, new_value) in self
            .runtime_objects_mut()
            .iter_mut()
            .zip(new_values.iter_mut())
        {
            let old_value = *slot;
            *slot = *new_value;
            *new_value = ptr::null_mut();
            if !old_value.is_null() {
                unsafe { ffi::Py_DECREF(old_value) };
            }
        }
        Ok(())
    }
}

impl Drop for FunctionEnv {
    fn drop(&mut self) {
        unsafe { cleanup_state_values(self.runtime_objects_mut()) };
        let globals_obj = self.globals_obj();
        if !globals_obj.is_null() {
            unsafe { ffi::Py_DECREF(globals_obj) };
            self.header_mut().globals_obj = ptr::null_mut();
        }
        let layout = Self::allocation_layout(self.runtime_object_len);
        unsafe { dealloc(self.abi.as_ptr() as *mut u8, layout) };
    }
}

impl PyFunctionJitExtra {
    fn function(&self) -> Result<&soac_core::block_py::BlockPyFunction<CodegenModuleShape>, ()> {
        self.module_state
            .lookup_function(self.function_id)
            .ok_or_else(|| unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"missing JIT function in registered module state".as_ptr(),
                );
            })
    }

    fn runtime_data_layout(&self) -> Result<jit::FunctionRuntimeDataLayout, ()> {
        Ok(jit::FunctionRuntimeDataLayout::from_function(
            self.function()?,
        ))
    }

    unsafe fn refresh_runtime_objects_after_function_update(
        &mut self,
        callable: *mut ffi::PyObject,
        event: PyFunctionWatchEvent,
        new_value: *mut ffi::PyObject,
    ) -> Result<(), ()> {
        let layout = self.runtime_data_layout()?;
        let defaults_override = (event == PY_FUNCTION_EVENT_MODIFY_DEFAULTS).then_some(new_value);
        let kwdefaults_override =
            (event == PY_FUNCTION_EVENT_MODIFY_KWDEFAULTS).then_some(new_value);
        let values = unsafe {
            collect_function_runtime_objects(
                callable,
                &layout,
                defaults_override,
                kwdefaults_override,
            )?
        };
        unsafe { self.function_env.replace_runtime_objects(values)? };
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct RegisteredFunctionOwnerTypes {
    function_weakref: usize,
    owner_type_weakrefs: Vec<usize>,
}

struct FunctionOwnerTypeRegistry {
    watcher_id: i32,
    registered_owner_types_by_function: Mutex<HashMap<usize, RegisteredFunctionOwnerTypes>>,
}

impl Drop for FunctionOwnerTypeRegistry {
    fn drop(&mut self) {
        if let Ok(mut weakrefs_by_function) = self.registered_owner_types_by_function.lock() {
            for registered in weakrefs_by_function.values_mut() {
                if registered.function_weakref != 0 {
                    unsafe { ffi::Py_DECREF(registered.function_weakref as *mut ffi::PyObject) };
                    registered.function_weakref = 0;
                }
                for weakref in registered.owner_type_weakrefs.drain(..) {
                    unsafe { ffi::Py_DECREF(weakref as *mut ffi::PyObject) };
                }
            }
        }
        let _ = self.watcher_id;
    }
}

static FUNCTION_OWNER_TYPE_REGISTRY: OnceLock<Result<FunctionOwnerTypeRegistry, ()>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub struct DirectMethodOwnerType {
    pub function_obj: *mut ffi::PyObject,
    pub owner_type: *mut ffi::PyTypeObject,
    pub type_version: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct DirectConstructorOwnerType {
    pub init_function_obj: *mut ffi::PyObject,
    pub owner_type: *mut ffi::PyTypeObject,
    pub type_version: u32,
}

fn set_type_error<T>(msg: &str) -> Result<T, ()> {
    unsafe {
        ffi::PyErr_SetString(ffi::PyExc_TypeError, CString::new(msg).unwrap().as_ptr());
    }
    Err(())
}

unsafe extern "C" fn free_clif_function_data(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(ptr as *mut PyFunctionJitExtra) });
}

unsafe fn py_unicode_utf8_str<'a>(obj: *mut ffi::PyObject) -> Result<&'a str, ()> {
    if ffi::PyUnicode_Check(obj) == 0 {
        return set_type_error("expected string keyword argument name in CLIF vectorcall binding");
    }
    let mut len = 0;
    let ptr = ffi::PyUnicode_AsUTF8AndSize(obj, &mut len);
    if ptr.is_null() {
        return Err(());
    }
    let bytes = std::slice::from_raw_parts(ptr as *const u8, len as usize);
    std::str::from_utf8(bytes).map_err(|_| {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"invalid UTF-8 keyword argument name in CLIF vectorcall binding".as_ptr(),
        );
    })
}

unsafe fn collect_function_runtime_objects(
    callable: *mut ffi::PyObject,
    layout: &jit::FunctionRuntimeDataLayout,
    defaults_override: Option<*mut ffi::PyObject>,
    kwdefaults_override: Option<*mut ffi::PyObject>,
) -> Result<Box<[*mut ffi::PyObject]>, ()> {
    if callable.is_null() || ffi::PyFunction_Check(callable) == 0 {
        return set_type_error("expected Python function while collecting CLIF function data");
    }

    let mut values = vec![ptr::null_mut(); layout.total_len()].into_boxed_slice();

    let defaults = defaults_override.unwrap_or_else(|| unsafe { PyFunction_GetDefaults(callable) });
    let default_len = if defaults.is_null() || ffi::PyTuple_Check(defaults) == 0 {
        0
    } else {
        unsafe { ffi::PyTuple_GET_SIZE(defaults) as usize }
    };
    let positional_count = layout.positional_default_count();
    let first_default_slot = positional_count.saturating_sub(default_len);
    let default_tuple_start = default_len.saturating_sub(positional_count);
    for default_slot in 0..positional_count {
        let value = if default_slot < first_default_slot || default_len == 0 {
            ptr::null_mut()
        } else {
            let tuple_index = default_tuple_start + default_slot - first_default_slot;
            if tuple_index >= default_len {
                ptr::null_mut()
            } else {
                unsafe { ffi::PyTuple_GetItem(defaults, tuple_index as ffi::Py_ssize_t) }
            }
        };
        if !value.is_null() {
            unsafe { ffi::Py_INCREF(value) };
        }
        values[layout.positional_default_slot(default_slot)] = value;
    }

    let kwdefaults =
        kwdefaults_override.unwrap_or_else(|| unsafe { PyFunction_GetKwDefaults(callable) });
    for (name, slot) in layout.kwonly_default_slots() {
        let Ok(name) = CString::new(name) else {
            continue;
        };
        let value = if kwdefaults.is_null() || ffi::PyDict_Check(kwdefaults) == 0 {
            ptr::null_mut()
        } else {
            unsafe { ffi::PyDict_GetItemString(kwdefaults, name.as_ptr()) }
        };
        if !value.is_null() {
            unsafe { ffi::Py_INCREF(value) };
        }
        values[slot] = value;
    }
    let closure = unsafe { PyFunction_GetClosure(callable) };
    for closure_slot in 0..layout.closure_len() {
        let value = if closure.is_null() || ffi::PyTuple_Check(closure) == 0 {
            ptr::null_mut()
        } else if closure_slot >= unsafe { ffi::PyTuple_GET_SIZE(closure) } as usize {
            ptr::null_mut()
        } else {
            unsafe { ffi::PyTuple_GetItem(closure, closure_slot as ffi::Py_ssize_t) }
        };
        if !value.is_null() {
            unsafe { ffi::Py_INCREF(value) };
        }
        values[layout.closure_cell_slot(closure_slot)] = value;
    }

    Ok(values)
}

pub unsafe fn clone_module_runtime_context(
    runtime: &jit::ModuleRuntimeContext,
) -> Result<jit::ModuleRuntimeContext, ()> {
    if runtime.mod_ctx.shared_module_state.is_null() || runtime.mod_ctx.globals_obj.is_null() {
        return set_runtime_error("cannot clone incomplete module runtime context");
    }
    unsafe {
        ffi::Py_INCREF(runtime.mod_ctx.globals_obj as *mut ffi::PyObject);
    }
    let shared_module_state_owner = runtime.shared_module_state_owner.clone();
    let compile_session = runtime.compile_session.clone();
    Ok(jit::ModuleRuntimeContext {
        mod_ctx: jit::ModuleJitContext {
            shared_module_state: std::sync::Arc::as_ptr(&shared_module_state_owner),
            globals_obj: runtime.mod_ctx.globals_obj,
        },
        compile_session,
        shared_module_state_owner,
    })
}

pub unsafe fn build_module_runtime_context_for_module(
    module: *mut ffi::PyObject,
) -> Result<jit::ModuleRuntimeContext, ()> {
    let py = Python::assume_attached();
    if module.is_null() {
        return set_runtime_error("missing transformed module while building runtime context");
    }
    let module = unsafe { Bound::from_borrowed_ptr(py, module) };
    let shared_module_state =
        crate::module_type::SoacExtModule::clone_shared_state(module.as_any()).map_err(|err| {
            err.restore(py);
        })?;
    let compile_session = CompileSession::process();
    let globals_obj = unsafe { ffi::PyModule_GetDict(module.as_ptr()) };
    if globals_obj.is_null() {
        if unsafe { ffi::PyErr_Occurred() }.is_null() {
            return set_runtime_error(
                "missing transformed module globals while building runtime context",
            );
        }
        return Err(());
    };
    unsafe { ffi::Py_INCREF(globals_obj) };
    Ok(jit::ModuleRuntimeContext {
        mod_ctx: jit::ModuleJitContext {
            shared_module_state: std::sync::Arc::as_ptr(&shared_module_state),
            globals_obj: globals_obj as *mut c_void,
        },
        compile_session,
        shared_module_state_owner: shared_module_state,
    })
}

pub unsafe fn start_background_jit_compile_for_module(
    module: *mut ffi::PyObject,
) -> Result<(), ()> {
    let py = Python::assume_attached();
    if module.is_null() {
        return set_runtime_error("missing transformed module for background JIT compile");
    }
    let module = unsafe { Bound::from_borrowed_ptr(py, module) };
    let shared_state = crate::module_type::SoacExtModule::clone_shared_state(module.as_any())
        .map_err(|err| {
            err.restore(py);
        })?;
    let compile_session = CompileSession::process();
    let process_jit = compile_session.process_jit().map_err(|err| {
        if let Ok(c_msg) = CString::new(err) {
            unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr()) };
        } else {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"failed to initialize process JIT for background compile\0".as_ptr()
                        as *const c_char,
                )
            };
        }
    })?;
    process_jit
        .start_background_compile_shared_module(Arc::clone(&compile_session), shared_state)
        .map_err(|err| {
            if let Ok(c_msg) = CString::new(err) {
                unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr()) };
            } else {
                unsafe {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to start background JIT compile\0".as_ptr() as *const c_char,
                    )
                };
            }
        })
}

unsafe fn make_clif_function_data(
    callable: *mut ffi::PyObject,
    function_id: RuntimeFunctionId,
    module_runtime: jit::ModuleRuntimeContext,
) -> Result<*mut c_void, ()> {
    let module_state = module_runtime.shared_module_state_owner.clone();
    let Some(blockpy_function) = module_state.lookup_function(function_id).cloned() else {
        let module_name = module_state.module_name.as_str();
        let msg = format!(
            "no specialized JIT plan found: module={module_name:?} function_id={function_id:?}"
        );
        if let Ok(c_msg) = CString::new(msg) {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"no specialized JIT plan found\0".as_ptr() as *const c_char,
            );
        }
        return Err(());
    };
    let runtime_data_layout = jit::FunctionRuntimeDataLayout::from_function(&blockpy_function);
    let binding_plan = DirectArgBindingPlan::from_function(&blockpy_function);
    let entry_plan = match jit::RuntimeFunctionEntryPlan::from_function(&blockpy_function) {
        Ok(plan) => plan,
        Err(err) => return set_runtime_error(&err),
    };
    let runtime_object_values =
        unsafe { collect_function_runtime_objects(callable, &runtime_data_layout, None, None)? };
    let mut function_env = unsafe {
        Box::new(FunctionEnv::new(
            module_runtime.mod_ctx.globals_obj as *mut ffi::PyObject,
            runtime_object_values,
        )?)
    };
    let function_env_ptr = function_env.as_mut_ptr();
    let py_function_extra = Box::new(PyFunctionJitExtra {
        function_env_ptr,
        function_id,
        function_env,
        binding_plan,
        entry_plan,
        compile_session: module_runtime.compile_session.clone(),
        module_state,
        compiled_vectorcall_entry: None,
        previous_vectorcall: unsafe { (*(callable as *mut ffi::PyFunctionObject)).vectorcall },
    });
    Ok(Box::into_raw(py_function_extra) as *mut c_void)
}

unsafe fn py_function_jit_extra(
    function: *mut ffi::PyObject,
) -> Result<&'static mut PyFunctionJitExtra, ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"expected Python function for CLIF vectorcall data lookup\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    let ptr = PyFunction_GetSoacMetadata(function);
    if ptr.is_null() {
        return set_runtime_error("missing CLIF vectorcall metadata");
    }
    Ok(&mut *(ptr as *mut PyFunctionJitExtra))
}

pub unsafe fn registered_clif_function_id(
    function: *mut ffi::PyObject,
) -> Result<Option<RuntimeFunctionId>, ()> {
    if ffi::PyFunction_Check(function) == 0 {
        return Ok(None);
    }
    let packed = PyFunction_GetSoacFunctionId(function);
    if packed == 0 {
        return Ok(None);
    }
    Ok(Some(RuntimeFunctionId::from_packed_runtime_u64(packed)))
}

pub unsafe fn registered_clif_type_function_id(
    type_obj: *mut ffi::PyObject,
) -> Result<Option<RuntimeFunctionId>, ()> {
    if ffi::PyType_Check(type_obj) == 0 {
        return Ok(None);
    }
    let type_obj = type_obj as *mut ffi::PyTypeObject;
    if ((*type_obj).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE) == 0 {
        return Ok(None);
    }
    let packed = PyType_GetSoacFunctionId(type_obj as *mut ffi::PyObject);
    if packed == 0 {
        return Ok(None);
    }
    Ok(Some(RuntimeFunctionId::from_packed_runtime_u64(packed)))
}

unsafe fn register_owner_type_for_function(
    function: *mut ffi::PyObject,
    owner_type: *mut ffi::PyTypeObject,
) -> Result<(), ()> {
    let registry = function_owner_type_registry()?;
    let function_key = function as usize;
    let mut owner_types_by_function =
        registry
            .registered_owner_types_by_function
            .lock()
            .map_err(|_| {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"function owner type registry lock poisoned".as_ptr(),
                );
            })?;
    let registered = owner_types_by_function
        .entry(function_key)
        .or_insert_with(|| {
            let function_weakref = PyWeakref_NewRef(function, ptr::null_mut());
            if function_weakref.is_null() {
                RegisteredFunctionOwnerTypes {
                    function_weakref: 0,
                    owner_type_weakrefs: Vec::new(),
                }
            } else {
                RegisteredFunctionOwnerTypes {
                    function_weakref: function_weakref as usize,
                    owner_type_weakrefs: Vec::new(),
                }
            }
        });
    if registered.function_weakref == 0 {
        return Err(());
    }
    for weakref in registered.owner_type_weakrefs.iter().copied() {
        let mut existing_owner = ptr::null_mut();
        match PyWeakref_GetRef(weakref as *mut ffi::PyObject, &mut existing_owner) {
            1 => {
                let matches = existing_owner == owner_type as *mut ffi::PyObject;
                ffi::Py_DECREF(existing_owner);
                if matches {
                    return Ok(());
                }
            }
            0 => {}
            _ => return Err(()),
        }
    }
    let owner_type_weakref = PyWeakref_NewRef(owner_type as *mut ffi::PyObject, ptr::null_mut());
    if owner_type_weakref.is_null() {
        return Err(());
    }
    registered
        .owner_type_weakrefs
        .push(owner_type_weakref as usize);
    Ok(())
}

unsafe fn incref_weakref_snapshot(weakref: usize) -> usize {
    ffi::Py_INCREF(weakref as *mut ffi::PyObject);
    weakref
}

unsafe fn resolve_weakref_target(weakref: usize) -> Result<Option<*mut ffi::PyObject>, ()> {
    let mut value = ptr::null_mut();
    match PyWeakref_GetRef(weakref as *mut ffi::PyObject, &mut value) {
        1 => Ok(Some(value)),
        0 => Ok(None),
        _ => Err(()),
    }
}

unsafe fn lookup_exact_owner_types_for_function_object(
    function_obj: *mut ffi::PyObject,
    method_name: &str,
    owner_type_weakrefs: &[usize],
) -> Result<Vec<DirectMethodOwnerType>, ()> {
    let method_name = CString::new(method_name).map_err(|_| {
        ffi::PyErr_SetString(
            ffi::PyExc_ValueError,
            c"method name for direct specialization contained NUL".as_ptr(),
        );
    })?;
    let mut out = Vec::new();
    for &owner_type_weakref in owner_type_weakrefs {
        let Some(owner_type_obj) = (unsafe { resolve_weakref_target(owner_type_weakref)? }) else {
            continue;
        };
        let owner_type = owner_type_obj as *mut ffi::PyTypeObject;
        let dict = (*owner_type).tp_dict;
        if !dict.is_null() {
            let current_descriptor = ffi::PyDict_GetItemString(dict, method_name.as_ptr());
            if current_descriptor == function_obj && ffi::PyFunction_Check(current_descriptor) != 0
            {
                if (*owner_type).tp_version_tag == 0 {
                    let _ = PyUnstable_Type_AssignVersionTag(owner_type);
                }
                let type_version = (*owner_type).tp_version_tag;
                if type_version != 0 {
                    out.push(DirectMethodOwnerType {
                        function_obj,
                        owner_type,
                        type_version,
                    });
                }
            }
        }
        ffi::Py_DECREF(owner_type_obj);
    }
    out.sort_by_key(|entry| (entry.owner_type as usize, entry.function_obj as usize));
    out.dedup_by_key(|entry| (entry.owner_type as usize, entry.function_obj as usize));
    Ok(out)
}

unsafe fn owner_type_has_simple_default_constructor(owner_type: *mut ffi::PyTypeObject) -> bool {
    if owner_type.is_null() {
        return false;
    }
    if ((*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE) == 0 {
        return false;
    }
    if ((*owner_type).tp_flags & ffi::Py_TPFLAGS_IS_ABSTRACT) != 0 {
        return false;
    }
    if ffi::Py_TYPE(owner_type as *mut ffi::PyObject) != std::ptr::addr_of_mut!(PyType_Type) {
        return false;
    }
    let Some(owner_tp_new) = (*owner_type).tp_new else {
        return false;
    };
    let Some(base_tp_new) = PyBaseObject_Type.tp_new else {
        return false;
    };
    if !std::ptr::fn_addr_eq(owner_tp_new, base_tp_new) {
        return false;
    }
    let Some(owner_tp_alloc) = (*owner_type).tp_alloc else {
        return false;
    };
    let generic_alloc: unsafe extern "C" fn(
        *mut ffi::PyTypeObject,
        ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject = PyType_GenericAlloc;
    if !std::ptr::fn_addr_eq(owner_tp_alloc, generic_alloc) {
        return false;
    }
    (*owner_type).tp_init.is_some()
}

unsafe fn lookup_exact_owner_types_for_constructor_object(
    function_obj: *mut ffi::PyObject,
    owner_type_weakrefs: &[usize],
) -> Result<Vec<DirectConstructorOwnerType>, ()> {
    let owners = lookup_exact_owner_types_for_function_object(
        function_obj,
        "__init__",
        owner_type_weakrefs,
    )?;
    let mut out = Vec::new();
    for owner in owners {
        if !owner_type_has_simple_default_constructor(owner.owner_type) {
            continue;
        }
        out.push(DirectConstructorOwnerType {
            init_function_obj: owner.function_obj,
            owner_type: owner.owner_type,
            type_version: owner.type_version,
        });
    }
    out.sort_by_key(|entry| (entry.owner_type as usize, entry.init_function_obj as usize));
    out.dedup_by_key(|entry| (entry.owner_type as usize, entry.init_function_obj as usize));
    Ok(out)
}

pub unsafe fn lookup_exact_owner_types_for_method(
    function_id: RuntimeFunctionId,
    method_name: &str,
) -> Result<Vec<DirectMethodOwnerType>, ()> {
    let Ok(registry) = function_owner_type_registry() else {
        return Ok(Vec::new());
    };
    let snapshot = {
        let registered_by_function =
            registry
                .registered_owner_types_by_function
                .lock()
                .map_err(|_| {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"function owner type registry lock poisoned".as_ptr(),
                    );
                })?;
        registered_by_function
            .values()
            .map(|registered| {
                (
                    unsafe { incref_weakref_snapshot(registered.function_weakref) },
                    registered
                        .owner_type_weakrefs
                        .iter()
                        .copied()
                        .map(|weakref| unsafe { incref_weakref_snapshot(weakref) })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };

    let mut out = Vec::new();
    for (function_weakref, owner_type_weakrefs) in snapshot {
        let Some(function_obj) = (unsafe { resolve_weakref_target(function_weakref)? }) else {
            ffi::Py_DECREF(function_weakref as *mut ffi::PyObject);
            for owner_type_weakref in owner_type_weakrefs {
                ffi::Py_DECREF(owner_type_weakref as *mut ffi::PyObject);
            }
            continue;
        };
        let matches_function_id = matches!(registered_clif_function_id(function_obj)?, Some(registered) if registered == function_id);
        if matches_function_id {
            let exact_owner_types = lookup_exact_owner_types_for_function_object(
                function_obj,
                method_name,
                owner_type_weakrefs.as_slice(),
            )?;
            for owner in exact_owner_types {
                if let Some(current_id) = registered_clif_function_id(owner.function_obj)? {
                    if current_id == function_id {
                        out.push(owner);
                    }
                }
            }
            for owner_type_weakref in owner_type_weakrefs {
                ffi::Py_DECREF(owner_type_weakref as *mut ffi::PyObject);
            }
        } else {
            for owner_type_weakref in owner_type_weakrefs {
                ffi::Py_DECREF(owner_type_weakref as *mut ffi::PyObject);
            }
        }
        ffi::Py_DECREF(function_obj);
        ffi::Py_DECREF(function_weakref as *mut ffi::PyObject);
    }
    Ok(out)
}

pub unsafe fn lookup_exact_owner_types_for_constructor(
    function_id: RuntimeFunctionId,
) -> Result<Vec<DirectConstructorOwnerType>, ()> {
    let Ok(registry) = function_owner_type_registry() else {
        return Ok(Vec::new());
    };
    let snapshot = {
        let registered_by_function =
            registry
                .registered_owner_types_by_function
                .lock()
                .map_err(|_| {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"function owner type registry lock poisoned".as_ptr(),
                    );
                })?;
        registered_by_function
            .values()
            .map(|registered| {
                (
                    unsafe { incref_weakref_snapshot(registered.function_weakref) },
                    registered
                        .owner_type_weakrefs
                        .iter()
                        .copied()
                        .map(|weakref| unsafe { incref_weakref_snapshot(weakref) })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };

    let mut out = Vec::new();
    for (function_weakref, owner_type_weakrefs) in snapshot {
        let Some(function_obj) = (unsafe { resolve_weakref_target(function_weakref)? }) else {
            ffi::Py_DECREF(function_weakref as *mut ffi::PyObject);
            for owner_type_weakref in owner_type_weakrefs {
                ffi::Py_DECREF(owner_type_weakref as *mut ffi::PyObject);
            }
            continue;
        };
        let matches_function_id = matches!(registered_clif_function_id(function_obj)?, Some(registered) if registered == function_id);
        if matches_function_id {
            let exact_owner_types = lookup_exact_owner_types_for_constructor_object(
                function_obj,
                owner_type_weakrefs.as_slice(),
            )?;
            for owner in exact_owner_types {
                if let Some(current_id) = registered_clif_function_id(owner.init_function_obj)? {
                    if current_id == function_id {
                        out.push(owner);
                    }
                }
            }
            for owner_type_weakref in owner_type_weakrefs {
                ffi::Py_DECREF(owner_type_weakref as *mut ffi::PyObject);
            }
        } else {
            for owner_type_weakref in owner_type_weakrefs {
                ffi::Py_DECREF(owner_type_weakref as *mut ffi::PyObject);
            }
        }
        ffi::Py_DECREF(function_obj);
        ffi::Py_DECREF(function_weakref as *mut ffi::PyObject);
    }
    Ok(out)
}

unsafe fn type_is_defined_in_module(
    owner_type: *mut ffi::PyTypeObject,
    module_name: *mut ffi::PyObject,
) -> bool {
    let owner_module =
        ffi::PyObject_GetAttrString(owner_type as *mut ffi::PyObject, c"__module__".as_ptr());
    if owner_module.is_null() {
        ffi::PyErr_Clear();
        return false;
    }
    let matches = ffi::PyObject_RichCompareBool(owner_module, module_name, ffi::Py_EQ);
    ffi::Py_DECREF(owner_module);
    if matches < 0 {
        ffi::PyErr_Clear();
        return false;
    }
    matches != 0
}

unsafe fn register_owner_types_from_type(
    owner_type: *mut ffi::PyTypeObject,
    module_name: *mut ffi::PyObject,
    visited_types: &mut HashSet<usize>,
) -> Result<(), ()> {
    if owner_type.is_null() || !visited_types.insert(owner_type as usize) {
        return Ok(());
    }
    if ((*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE) == 0 {
        return Ok(());
    }
    if !type_is_defined_in_module(owner_type, module_name) {
        return Ok(());
    }
    if PyType_SetSoacMetadata(owner_type as *mut ffi::PyObject, 0, ptr::null_mut(), None) != 0 {
        return Err(());
    }
    let dict = (*owner_type).tp_dict;
    if dict.is_null() {
        return Ok(());
    }
    let mut constructor_function_id = None;
    let mut pos: ffi::Py_ssize_t = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while ffi::PyDict_Next(dict, &mut pos, &mut key, &mut value) != 0 {
        if ffi::PyFunction_Check(value) != 0 {
            if ffi::PyUnicode_Check(key) != 0
                && ffi::PyUnicode_CompareWithASCIIString(key, c"__init__".as_ptr()) == 0
            {
                constructor_function_id = registered_clif_function_id(value)?;
            }
            register_owner_type_for_function(value, owner_type)?;
        } else if ffi::PyType_Check(value) != 0 {
            register_owner_types_from_type(
                value as *mut ffi::PyTypeObject,
                module_name,
                visited_types,
            )?;
        }
    }
    if let Some(function_id) = constructor_function_id {
        if PyType_SetSoacMetadata(
            owner_type as *mut ffi::PyObject,
            function_id.to_packed_runtime_u64(),
            ptr::null_mut(),
            None,
        ) != 0
        {
            return Err(());
        }
    }
    Ok(())
}

unsafe fn register_function_owner_type_value(
    value: *mut ffi::PyObject,
    module_name: *mut ffi::PyObject,
    visited_types: &mut HashSet<usize>,
) -> Result<(), ()> {
    if ffi::PyType_Check(value) != 0 {
        register_owner_types_from_type(
            value as *mut ffi::PyTypeObject,
            module_name,
            visited_types,
        )?;
    }
    Ok(())
}

unsafe fn register_function_owner_type_indexed_key(
    globals: *mut ffi::PyObject,
    module_name: *mut ffi::PyObject,
    key: &str,
    visited_types: &mut HashSet<usize>,
) -> Result<(), ()> {
    let key_obj = ffi::PyUnicode_FromStringAndSize(
        key.as_ptr().cast::<c_char>(),
        key.len() as ffi::Py_ssize_t,
    );
    if key_obj.is_null() {
        return Err(());
    }
    let index = _PyDict_IndexedKeyIndex(globals, key_obj);
    ffi::Py_DECREF(key_obj);
    if index < 0 {
        ffi::PyErr_Clear();
        return Ok(());
    }

    let mut value = ptr::null_mut();
    let found = _PyDict_GetIndexedItem(globals, index, &mut value);
    if found < 0 {
        ffi::PyErr_Clear();
        return Ok(());
    }
    if found == 0 {
        return Ok(());
    }
    let result = register_function_owner_type_value(value, module_name, visited_types);
    ffi::Py_DECREF(value);
    result
}

unsafe fn register_function_owner_types_for_globals(
    globals: *mut ffi::PyObject,
    module_name: *mut ffi::PyObject,
    indexed_module_keys: &[String],
) -> Result<(), ()> {
    if globals.is_null() {
        if ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"module globals missing while registering owner types".as_ptr(),
            );
        }
        return Err(());
    }
    if module_name.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"module name missing while registering owner types".as_ptr(),
        );
        return Err(());
    }
    let mut visited_types = HashSet::new();
    let mut pos: ffi::Py_ssize_t = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while ffi::PyDict_Next(globals, &mut pos, &mut key, &mut value) != 0 {
        register_function_owner_type_value(value, module_name, &mut visited_types)?;
    }
    for key in indexed_module_keys {
        register_function_owner_type_indexed_key(
            globals,
            module_name,
            key.as_str(),
            &mut visited_types,
        )?;
    }
    Ok(())
}

pub unsafe fn register_function_owner_types_for_module(
    module: *mut ffi::PyObject,
) -> Result<(), ()> {
    if module.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"register_function_owner_types_for_module requires a module".as_ptr(),
        );
        return Err(());
    }
    let globals = ffi::PyModule_GetDict(module);
    let module_name = if globals.is_null() {
        ptr::null_mut()
    } else {
        ffi::PyDict_GetItemString(globals, c"__name__".as_ptr())
    };
    register_function_owner_types_for_globals(globals, module_name, &[])
}

pub unsafe fn register_function_owner_types_for_module_keys(
    module: *mut ffi::PyObject,
    indexed_module_keys: &[String],
) -> Result<(), ()> {
    if module.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"register_function_owner_types_for_module_keys requires a module".as_ptr(),
        );
        return Err(());
    }
    let globals = ffi::PyModule_GetDict(module);
    let module_name = if globals.is_null() {
        ptr::null_mut()
    } else {
        ffi::PyDict_GetItemString(globals, c"__name__".as_ptr())
    };
    register_function_owner_types_for_globals(globals, module_name, indexed_module_keys)
}

unsafe fn ensure_clif_direct_entries_compiled(
    _py: Python<'_>,
    data: &mut PyFunctionJitExtra,
) -> Result<(), ()> {
    if data.function_env.compiled_function.is_none() {
        let function = data
            .function()
            .map(soac_core::block_py::BlockPyFunction::clone)?;
        let ensure_start = Instant::now();
        let function_block_count = function.blocks.len();
        let function_qualname = function.names.qualname.clone();
        let module_state = Arc::clone(&data.module_state);
        let compile_session = Arc::clone(&data.compile_session);
        let compile_start = Instant::now();
        let compiled_function_result = match module_state
            .lookup_or_compile_direct_function_handle(&compile_session, function.function_id)
        {
            Ok(Some((handle, compiled))) => {
                if !compiled
                    && !jit::is_synthetic_class_helper_function(&function)
                    && let Some(stats) = handle.jit_stats()
                {
                    module_state.append_jit_codegen_log(
                        &function,
                        "direct_function_body",
                        compile_start.elapsed(),
                        "ok",
                        None,
                        Some(stats),
                    );
                }
                Ok(handle)
            }
            Ok(None) => {
                let block_ptrs = vec![ptr::null_mut::<c_void>(); function.blocks.len()];
                let module_constant_ptrs = module_state.module_constant_ptrs();
                let compile_result = jit::compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    block_ptrs.as_slice(),
                    &module_state.lowered_module,
                    &function,
                    &module_state.codegen_constants,
                    &module_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(module_state.as_ref()),
                );
                match compile_result {
                    Ok(result) => {
                        if result.compiled {
                            module_state.append_jit_codegen_log(
                                &function,
                                "vectorcall_function_body",
                                compile_start.elapsed(),
                                "ok",
                                None,
                                result.stats.as_ref(),
                            );
                        }
                        Ok(result.handle)
                    }
                    Err(err) => {
                        module_state.append_jit_codegen_log(
                            &function,
                            "vectorcall_function_body",
                            compile_start.elapsed(),
                            "error",
                            Some(&err),
                            None,
                        );
                        Err(err)
                    }
                }
            }
            Err(err) => Err(err),
        };
        let compiled_function = match compiled_function_result {
            Ok(handle) => handle,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to compile CLIF function body\0".as_ptr() as *const i8,
                    );
                }
                return Err(());
            }
        };
        let direct_code_ptr = match compiled_function
            .direct_code_ptr()
            .map(|ptr| ptr as *const u8)
        {
            Ok(ptr) => ptr,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"missing CLIF direct entry\0".as_ptr() as *const i8,
                    );
                }
                return Err(());
            }
        };
        data.function_env.set_direct_code_ptr(direct_code_ptr);
        let default_direct_code_ptr = match compiled_function
            .default_direct_code_ptr()
            .map(|ptr| ptr as *const u8)
        {
            Ok(ptr) => ptr,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"missing CLIF default direct entry\0".as_ptr() as *const i8,
                    );
                }
                return Err(());
            }
        };
        data.function_env
            .set_default_direct_code_ptr(default_direct_code_ptr);
        let deopt_table_ptr = match compiled_function.direct_deopt_table_ptr() {
            Ok(ptr) => ptr as *const c_void,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"missing CLIF deopt metadata\0".as_ptr() as *const i8,
                    );
                }
                return Err(());
            }
        };
        data.function_env.set_deopt_table_ptr(deopt_table_ptr);
        data.function_env.compiled_function = Some(compiled_function);
        let elapsed_ms = ensure_start.elapsed().as_secs_f64() * 1000.0;
        info!(
            "soac_jit_precompile module={} qualname={} blocks={} elapsed_ms={elapsed_ms:.3}",
            data.module_state.module_name, function_qualname, function_block_count,
        );
    }
    if data.function_env.direct_code_ptr().is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"compiled CLIF function is missing a direct entry pointer\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    if data.function_env.default_direct_code_ptr().is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"compiled CLIF function is missing a default direct entry pointer\0".as_ptr()
                as *const i8,
        );
        return Err(());
    }
    if data.function_env.deopt_table_ptr().is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"compiled CLIF function is missing deopt metadata\0".as_ptr() as *const i8,
        );
        return Err(());
    }
    Ok(())
}

unsafe fn ensure_clif_vectorcall_compiled(
    py: Python<'_>,
    callable: *mut ffi::PyObject,
    data: &mut PyFunctionJitExtra,
) -> Result<(), ()> {
    unsafe { ensure_clif_direct_entries_compiled(py, data)? };
    if data.compiled_vectorcall_entry.is_none() {
        let param_count = data.binding_plan.param_count();
        let entry = match data
            .compile_session
            .process_jit()
            .and_then(|engine| engine.vectorcall_trampoline(&data.compile_session, param_count))
        {
            Ok(value) => value,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to compile direct CLIF vectorcall trampoline\0".as_ptr()
                            as *const i8,
                    );
                }
                return Err(());
            }
        };
        data.compiled_vectorcall_entry = Some(entry);
        let vectorcall_entry: ffi::vectorcallfunc = std::mem::transmute(entry);
        PyFunction_SetVectorcall(
            callable as *mut ffi::PyFunctionObject,
            Some(vectorcall_entry),
        );
    }
    Ok(())
}

unsafe fn cleanup_state_values(state_values: &mut [*mut ffi::PyObject]) {
    for value in state_values.iter_mut() {
        if !value.is_null() {
            ffi::Py_DECREF(*value);
            *value = ptr::null_mut();
        }
    }
}

unsafe fn cleanup_output_args(out_args: *mut *mut ffi::PyObject, out_len: usize) {
    if out_args.is_null() {
        return;
    }
    for index in 0..out_len {
        let slot = out_args.add(index);
        let value = *slot;
        if !value.is_null() {
            ffi::Py_DECREF(value);
            *slot = ptr::null_mut();
        }
    }
}

unsafe fn initialize_output_args(
    out_args: *mut *mut ffi::PyObject,
    out_len: usize,
) -> Result<(), ()> {
    if out_len == 0 {
        return Ok(());
    }
    if out_args.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"missing output buffer for direct CLIF function arguments".as_ptr(),
        );
        return Err(());
    }
    for index in 0..out_len {
        *out_args.add(index) = ptr::null_mut();
    }
    Ok(())
}

unsafe fn output_arg_is_assigned(out_args: *mut *mut ffi::PyObject, param_index: usize) -> bool {
    !(*out_args.add(param_index)).is_null()
}

unsafe fn write_output_arg_from_borrowed(
    out_args: *mut *mut ffi::PyObject,
    param_index: usize,
    value: *mut ffi::PyObject,
) {
    ffi::Py_INCREF(value);
    *out_args.add(param_index) = value;
}

unsafe fn write_output_arg_from_owned(
    out_args: *mut *mut ffi::PyObject,
    param_index: usize,
    value: *mut ffi::PyObject,
) {
    *out_args.add(param_index) = value;
}

fn binding_type_error<T>(msg: String) -> Result<T, ()> {
    let _ = set_type_error::<()>(&msg);
    Err(())
}

unsafe fn bind_function_args_to_output(
    data: &PyFunctionJitExtra,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
    out_args: *mut *mut ffi::PyObject,
    out_len: usize,
) -> Result<(), ()> {
    let plan = &data.binding_plan;
    if out_len != plan.param_count() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"bound CLIF argument count did not match direct entry arity".as_ptr(),
        );
        return Err(());
    }
    initialize_output_args(out_args, out_len)?;
    let callable_name = plan.callable_name.as_str();
    let nargs = ffi::PyVectorcall_NARGS(nargsf) as usize;
    let nkw = if kwnames.is_null() {
        0
    } else {
        ffi::PyTuple_GET_SIZE(kwnames) as usize
    };
    if (nargs > 0 || nkw > 0) && args.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"missing vectorcall argument array in CLIF function binding".as_ptr(),
        );
        return Err(());
    }

    let positional_capacity = plan.positional_capacity();
    if plan.varargs_param.is_none() && nargs > positional_capacity {
        return binding_type_error(format!(
            "{}() takes {} positional argument{} but {} {} given",
            callable_name,
            positional_capacity,
            if positional_capacity == 1 { "" } else { "s" },
            nargs,
            if nargs == 1 { "was" } else { "were" }
        ));
    }

    let positional_bound = nargs.min(positional_capacity);
    for position in 0..positional_bound {
        let param_index = plan.positional_param_indices[position];
        let value = *args.add(position);
        if value.is_null() {
            cleanup_output_args(out_args, out_len);
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"null vectorcall positional argument\0".as_ptr() as *const i8,
            );
            return Err(());
        }
        write_output_arg_from_borrowed(out_args, param_index, value);
    }

    if let Some(varargs_param) = plan.varargs_param {
        let extras = nargs.saturating_sub(positional_capacity);
        let extra_tuple = ffi::PyTuple_New(extras as ffi::Py_ssize_t);
        if extra_tuple.is_null() {
            cleanup_output_args(out_args, out_len);
            return Err(());
        }
        for offset in 0..extras {
            let value = *args.add(positional_capacity + offset);
            if value.is_null() {
                ffi::Py_DECREF(extra_tuple);
                cleanup_output_args(out_args, out_len);
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"null vectorcall positional vararg\0".as_ptr() as *const i8,
                );
                return Err(());
            }
            ffi::Py_INCREF(value);
            if ffi::PyTuple_SetItem(extra_tuple, offset as ffi::Py_ssize_t, value) != 0 {
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(extra_tuple);
                cleanup_output_args(out_args, out_len);
                return Err(());
            }
        }
        write_output_arg_from_owned(out_args, varargs_param, extra_tuple);
    }

    let has_varkw = plan.varkw_param.is_some();
    let mut varkw_dict = ptr::null_mut();
    if let Some(varkw_param) = plan.varkw_param {
        varkw_dict = ffi::PyDict_New();
        if varkw_dict.is_null() {
            cleanup_output_args(out_args, out_len);
            return Err(());
        }
        write_output_arg_from_owned(out_args, varkw_param, varkw_dict);
    }

    for kw_index in 0..nkw {
        let key = ffi::PyTuple_GetItem(kwnames, kw_index as ffi::Py_ssize_t);
        if key.is_null() {
            cleanup_output_args(out_args, out_len);
            return Err(());
        }
        let value = *args.add(nargs + kw_index);
        if value.is_null() {
            cleanup_output_args(out_args, out_len);
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"null vectorcall keyword argument\0".as_ptr() as *const i8,
            );
            return Err(());
        }
        let key_name = match py_unicode_utf8_str(key) {
            Ok(name) => name,
            Err(()) => {
                cleanup_output_args(out_args, out_len);
                return Err(());
            }
        };
        if let Some(param_index) = plan.param_index(key_name) {
            let param = &plan.params[param_index];
            match param.kind {
                ParamKind::PosOnly | ParamKind::VarArg => {
                    if !has_varkw {
                        cleanup_output_args(out_args, out_len);
                        return binding_type_error(format!(
                            "{}() got an unexpected keyword argument '{}'",
                            callable_name, key_name
                        ));
                    }
                    if !varkw_dict.is_null() && ffi::PyDict_SetItem(varkw_dict, key, value) != 0 {
                        cleanup_output_args(out_args, out_len);
                        return Err(());
                    }
                }
                ParamKind::Any | ParamKind::KwOnly => {
                    if output_arg_is_assigned(out_args, param_index) {
                        cleanup_output_args(out_args, out_len);
                        return binding_type_error(format!(
                            "{}() got multiple values for argument '{}'",
                            callable_name, key_name
                        ));
                    }
                    write_output_arg_from_borrowed(out_args, param_index, value);
                }
                ParamKind::KwArg => {
                    if !varkw_dict.is_null() && ffi::PyDict_SetItem(varkw_dict, key, value) != 0 {
                        cleanup_output_args(out_args, out_len);
                        return Err(());
                    }
                }
            }
        } else if has_varkw {
            if !varkw_dict.is_null() && ffi::PyDict_SetItem(varkw_dict, key, value) != 0 {
                cleanup_output_args(out_args, out_len);
                return Err(());
            }
        } else {
            cleanup_output_args(out_args, out_len);
            return binding_type_error(format!(
                "{}() got an unexpected keyword argument '{}'",
                callable_name, key_name
            ));
        }
    }

    for (param_index, param) in plan.params.iter().enumerate() {
        if output_arg_is_assigned(out_args, param_index) {
            continue;
        }
        match param.kind {
            ParamKind::VarArg | ParamKind::KwArg => {}
            _ => {
                if param
                    .default_slot
                    .is_some_and(|slot| !data.function_env.runtime_object(slot).is_null())
                {
                    continue;
                }
                cleanup_output_args(out_args, out_len);
                return binding_type_error(format!(
                    "{}() missing required argument '{}'",
                    callable_name, param.name
                ));
            }
        }
    }
    Ok(())
}

pub(crate) unsafe extern "C" fn bind_direct_args_from_vectorcall(
    callable: *mut c_void,
    args: *const *mut c_void,
    nargsf: usize,
    kwnames: *mut c_void,
    data_ptr: *mut c_void,
    out_args: *mut *mut c_void,
    out_len: i64,
) -> i32 {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        if callable.is_null() || data_ptr.is_null() || out_len < 0 {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"invalid direct vectorcall bind input\0".as_ptr() as *const i8,
            );
            return 0;
        }
        let data = &mut *(data_ptr as *mut PyFunctionJitExtra);
        match bind_function_args_to_output(
            data,
            args as *const *mut ffi::PyObject,
            nargsf,
            kwnames as *mut ffi::PyObject,
            out_args as *mut *mut ffi::PyObject,
            out_len as usize,
        ) {
            Ok(()) => 1,
            Err(()) => 0,
        }
    })) {
        Ok(value) => value,
        Err(payload) => {
            let message = format!(
                "panic in bind_direct_args_from_vectorcall: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"panic in bind_direct_args_from_vectorcall\0".as_ptr() as *const i8,
                );
            }
            0
        }
    }
}

pub(crate) unsafe extern "C" fn vectorcall_compile_function_env(
    callable: *mut c_void,
    data_ptr: *mut c_void,
) -> *mut c_void {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        if callable.is_null()
            || data_ptr.is_null()
            || ffi::PyFunction_Check(callable as *mut ffi::PyObject) == 0
        {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"invalid vectorcall function env compile input\0".as_ptr() as *const i8,
            );
            return ptr::null_mut();
        }
        let py = Python::assume_attached();
        let data = &mut *(data_ptr as *mut PyFunctionJitExtra);
        match ensure_clif_vectorcall_compiled(py, callable as *mut ffi::PyObject, data) {
            Ok(()) => data.function_env_ptr,
            Err(()) => ptr::null_mut(),
        }
    })) {
        Ok(value) => value,
        Err(payload) => {
            let message = format!(
                "panic in vectorcall_compile_function_env: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"panic in vectorcall_compile_function_env\0".as_ptr() as *const i8,
                );
            }
            ptr::null_mut()
        }
    }
}

pub(crate) unsafe extern "C" fn direct_compile_function_env(
    callable: *mut c_void,
    data_ptr: *mut c_void,
) -> *mut c_void {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        if callable.is_null()
            || data_ptr.is_null()
            || ffi::PyFunction_Check(callable as *mut ffi::PyObject) == 0
        {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"invalid direct function env compile input\0".as_ptr() as *const i8,
            );
            return ptr::null_mut();
        }
        let py = Python::assume_attached();
        let data = &mut *(data_ptr as *mut PyFunctionJitExtra);
        match ensure_clif_direct_entries_compiled(py, data) {
            Ok(()) => data.function_env_ptr,
            Err(()) => ptr::null_mut(),
        }
    })) {
        Ok(value) => value,
        Err(payload) => {
            let message = format!(
                "panic in direct_compile_function_env: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"panic in direct_compile_function_env\0".as_ptr() as *const i8,
                );
            }
            ptr::null_mut()
        }
    }
}

pub(crate) unsafe fn register_clif_direct_metadata(
    function: *mut ffi::PyObject,
    function_id: RuntimeFunctionId,
    module_runtime: jit::ModuleRuntimeContext,
) -> Result<(), ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"register_clif_direct_metadata expects a Python function\0".as_ptr() as *const c_char,
        );
        return Err(());
    }
    let _watcher = function_owner_type_registry()?;
    let data_ptr = make_clif_function_data(function, function_id, module_runtime)?;
    if PyFunction_SetSoacMetadata(
        function,
        function_id.to_packed_runtime_u64(),
        data_ptr,
        Some(free_clif_function_data),
    ) != 0
    {
        free_clif_function_data(data_ptr);
        return Err(());
    }
    Ok(())
}

pub unsafe fn register_clif_vectorcall(
    function: *mut ffi::PyObject,
    function_id: RuntimeFunctionId,
    module_runtime: jit::ModuleRuntimeContext,
) -> Result<(), ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"register_clif_vectorcall expects a Python function\0".as_ptr() as *const c_char,
        );
        return Err(());
    }
    let func = function as *mut ffi::PyFunctionObject;
    if !PyFunction_GetSoacMetadata(function).is_null() {
        let data = unsafe { py_function_jit_extra(function)? };
        if entry_interpreter_vectorcall_requested(data.function()?) {
            data.compiled_vectorcall_entry = None;
            PyFunction_SetVectorcall(func, Some(entry_interpreter_vectorcall));
            return Ok(());
        }
        let param_count = data.binding_plan.param_count();
        let entry = data
            .compile_session
            .process_jit()
            .and_then(|engine| engine.vectorcall_trampoline(&data.compile_session, param_count))
            .map_err(|err| {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to compile shared CLIF vectorcall trampoline\0".as_ptr()
                            as *const i8,
                    );
                }
            })?;
        data.compiled_vectorcall_entry = Some(entry);
        let vectorcall_entry: ffi::vectorcallfunc = std::mem::transmute(entry);
        PyFunction_SetVectorcall(func, Some(vectorcall_entry));
        return Ok(());
    }
    let _watcher = function_owner_type_registry()?;
    let blockpy_function = module_runtime
        .shared_module_state_owner
        .lookup_function(function_id)
        .ok_or_else(|| {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"no specialized JIT plan found while registering vectorcall".as_ptr(),
            );
        })?;
    let blockpy_function_kind = *blockpy_function.lowered_kind();
    let blockpy_function_param_count = blockpy_function.params.len();
    if entry_interpreter_vectorcall_requested(blockpy_function) {
        let data_ptr = make_clif_function_data(function, function_id, module_runtime)?;
        if PyFunction_SetSoacMetadata(
            function,
            function_id.to_packed_runtime_u64(),
            data_ptr,
            Some(free_clif_function_data),
        ) != 0
        {
            free_clif_function_data(data_ptr);
            return Err(());
        }
        PyFunction_SetVectorcall(func, Some(entry_interpreter_vectorcall));
        return Ok(());
    }
    if entry_interpreter_vectorcall_for_tests_enabled() {
        let data_ptr = make_clif_function_data(function, function_id, module_runtime)?;
        if PyFunction_SetSoacMetadata(
            function,
            function_id.to_packed_runtime_u64(),
            data_ptr,
            Some(free_clif_function_data),
        ) != 0
        {
            free_clif_function_data(data_ptr);
            return Err(());
        }
        if blockpy_function_kind == FunctionKind::Function {
            PyFunction_SetVectorcall(func, Some(entry_interpreter_vectorcall));
        }
        return Ok(());
    }
    let entry = module_runtime
        .compile_session
        .process_jit()
        .and_then(|engine| {
            engine.vectorcall_trampoline(
                &module_runtime.compile_session,
                blockpy_function_param_count,
            )
        })
        .map_err(|err| {
            if let Ok(c_msg) = CString::new(err) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"failed to compile shared CLIF vectorcall trampoline\0".as_ptr() as *const i8,
                );
            }
        })?;

    let data_ptr = make_clif_function_data(function, function_id, module_runtime)?;
    let data = unsafe { &mut *(data_ptr as *mut PyFunctionJitExtra) };
    data.compiled_vectorcall_entry = Some(entry);
    if PyFunction_SetSoacMetadata(
        function,
        function_id.to_packed_runtime_u64(),
        data_ptr,
        Some(free_clif_function_data),
    ) != 0
    {
        free_clif_function_data(data_ptr);
        return Err(());
    }
    let vectorcall_entry: ffi::vectorcallfunc = std::mem::transmute(entry);
    PyFunction_SetVectorcall(func, Some(vectorcall_entry));
    Ok(())
}

pub unsafe fn compile_clif_vectorcall(function: *mut ffi::PyObject) -> Result<(), ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"compile_clif_vectorcall expects a Python function\0".as_ptr() as *const c_char,
        );
        return Err(());
    }
    let py = Python::assume_attached();
    let data = py_function_jit_extra(function)?;
    ensure_clif_vectorcall_compiled(py, function, data)
}

pub fn force_entry_interpreter_vectorcall_for_tests(enabled: bool) -> bool {
    FORCE_ENTRY_INTERPRETER_VECTORCALL_FOR_TESTS.swap(enabled, Ordering::SeqCst)
}

fn entry_interpreter_vectorcall_for_tests_enabled() -> bool {
    FORCE_ENTRY_INTERPRETER_VECTORCALL_FOR_TESTS.load(Ordering::SeqCst)
}

fn entry_interpreter_vectorcall_requested(
    function: &soac_core::block_py::BlockPyFunction<CodegenModuleShape>,
) -> bool {
    function.execution_mode() == FunctionExecutionMode::Interpreted
        || (entry_interpreter_vectorcall_for_tests_enabled()
            && *function.lowered_kind() == FunctionKind::Function)
}

struct EntryInterpreterRecursiveCallGuard;

impl Drop for EntryInterpreterRecursiveCallGuard {
    fn drop(&mut self) {
        unsafe { ffi::Py_LeaveRecursiveCall() };
    }
}

unsafe extern "C" fn entry_interpreter_vectorcall(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if ffi::Py_EnterRecursiveCall(c" while calling a Python object".as_ptr()) != 0 {
        return ptr::null_mut();
    }
    let _recursive_call = EntryInterpreterRecursiveCallGuard;
    match panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        run_registered_clif_function_from_vectorcall_entry(callable, args, nargsf, kwnames)
    })) {
        Ok(Ok(result)) => result,
        Ok(Err(())) => ptr::null_mut(),
        Err(payload) => {
            let message = format!(
                "panic in entry_interpreter_vectorcall: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"panic in entry_interpreter_vectorcall".as_ptr(),
                );
            }
            ptr::null_mut()
        }
    }
}

#[cold]
#[allow(dead_code)]
pub(crate) unsafe fn run_registered_clif_function_from_vectorcall_entry(
    function_obj: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> Result<*mut ffi::PyObject, ()> {
    if ffi::PyFunction_Check(function_obj) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            c"entry interpreter expects a registered Python function".as_ptr(),
        );
        return Err(());
    }
    let data = py_function_jit_extra(function_obj)?;
    let blockpy_function = data.function()?;
    let context = jit::BlockPyEntryRuntimeContext::new(
        Arc::clone(&data.compile_session),
        Arc::clone(&data.module_state),
        data.function_env.globals_obj().cast::<c_void>(),
        data.function_env.runtime_objects_ptr().cast::<c_void>(),
        &data.entry_plan,
    );
    match jit::run_blockpy_function_from_vectorcall_entry(
        blockpy_function,
        context,
        args.cast::<*mut c_void>(),
        nargsf,
        kwnames.cast::<c_void>(),
    ) {
        Ok(result) => Ok(result.cast::<ffi::PyObject>()),
        Err(err) => {
            if let Ok(c_msg) = CString::new(err) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"entry interpreter failed".as_ptr(),
                );
            }
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyDict, PyList, PyModule};
    use std::sync::atomic::{AtomicUsize, Ordering};

    unsafe extern "C" {
        fn PyUnstable_Type_AssignVersionTag(type_obj: *mut ffi::PyTypeObject) -> i32;
        fn PyFunction_SetDefaults(
            function: *mut ffi::PyObject,
            defaults: *mut ffi::PyObject,
        ) -> i32;
        fn PyFunction_SetKwDefaults(
            function: *mut ffi::PyObject,
            defaults: *mut ffi::PyObject,
        ) -> i32;
    }

    unsafe fn make_test_module_with_source<'py>(
        py: Python<'py>,
        source: &str,
    ) -> (Bound<'py, PyModule>, Bound<'py, PyAny>) {
        let module = PyModule::new(py, "watcher_test").expect("module should allocate");
        let globals = module.dict();
        let locals = PyDict::new(py);
        let builtins = py.import("builtins").expect("builtins should import");
        globals
            .set_item("__builtins__", &builtins)
            .expect("module globals should accept builtins");
        locals
            .set_item("__builtins__", &builtins)
            .expect("locals should accept builtins");

        let source =
            std::ffi::CString::new(source).expect("python source should be CString-compatible");
        assert!(
            !ffi::PyRun_StringFlags(
                source.as_ptr(),
                ffi::Py_file_input,
                globals.as_ptr(),
                locals.as_ptr(),
                ptr::null_mut(),
            )
            .is_null(),
            "class definition should execute"
        );

        let cls = locals
            .get_item("C")
            .expect("locals lookup should succeed")
            .expect("class should exist");
        globals
            .set_item("C", &cls)
            .expect("module globals should accept class");
        (module, cls)
    }

    unsafe fn make_test_module<'py>(py: Python<'py>) -> (Bound<'py, PyModule>, Bound<'py, PyAny>) {
        make_test_module_with_source(
            py,
            "class C:\n    def __init__(self):\n        self.value = 1\n    def f(self, x=1, *, y=2):\n        return x + y\n",
        )
    }

    unsafe fn assert_function_mutation_invalidates_type_version(
        owner_type: *mut ffi::PyTypeObject,
        mutation_name: &str,
        mutate: impl FnOnce() -> i32,
    ) {
        assert_eq!(
            PyUnstable_Type_AssignVersionTag(owner_type),
            1,
            "{mutation_name}: class should receive a version tag"
        );
        let before = (*owner_type).tp_version_tag;
        assert_ne!(
            before, 0,
            "{mutation_name}: type version tag should be assigned"
        );
        assert_eq!(mutate(), 0, "{mutation_name}: mutation should succeed");
        assert_ne!(
            (*owner_type).tp_version_tag,
            before,
            "{mutation_name}: function mutation should invalidate the owner type version"
        );
    }

    unsafe fn compile_replacement_function(py: Python<'_>) -> *mut ffi::PyObject {
        let globals = PyDict::new(py);
        let locals = PyDict::new(py);
        let builtins = py.import("builtins").expect("builtins should import");
        globals
            .set_item("__builtins__", &builtins)
            .expect("replacement globals should accept builtins");
        locals
            .set_item("__builtins__", &builtins)
            .expect("replacement locals should accept builtins");
        let source =
            std::ffi::CString::new("def replacement(self, x=1, *, y=2):\n    return x - y\n")
                .expect("replacement source should be CString-compatible");
        let run_result = ffi::PyRun_StringFlags(
            source.as_ptr(),
            ffi::Py_file_input,
            globals.as_ptr(),
            locals.as_ptr(),
            ptr::null_mut(),
        );
        assert!(
            !run_result.is_null(),
            "replacement function definition should execute"
        );
        ffi::Py_DECREF(run_result);
        let replacement = locals
            .get_item("replacement")
            .expect("replacement lookup should succeed")
            .expect("replacement function should exist");
        let replacement_code =
            ffi::PyObject_GetAttrString(replacement.as_ptr(), c"__code__".as_ptr());
        assert!(
            !replacement_code.is_null(),
            "replacement function should expose __code__"
        );
        replacement_code
    }

    static TEST_SOAC_METADATA_DROPS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn free_test_soac_metadata(ptr: *mut c_void) {
        if !ptr.is_null() {
            drop(Box::from_raw(ptr as *mut usize));
        }
        TEST_SOAC_METADATA_DROPS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe fn make_test_function(py: Python<'_>) -> *mut ffi::PyObject {
        let globals = PyDict::new(py);
        let locals = PyDict::new(py);
        let builtins = py.import("builtins").expect("builtins should import");
        globals
            .set_item("__builtins__", &builtins)
            .expect("globals should accept builtins");
        locals
            .set_item("__builtins__", &builtins)
            .expect("locals should accept builtins");
        let source = std::ffi::CString::new("def f():\n    return 1\n")
            .expect("test function source should be CString-compatible");
        let run_result = ffi::PyRun_StringFlags(
            source.as_ptr(),
            ffi::Py_file_input,
            globals.as_ptr(),
            locals.as_ptr(),
            ptr::null_mut(),
        );
        assert!(
            !run_result.is_null(),
            "test function definition should execute"
        );
        ffi::Py_DECREF(run_result);
        let function = locals
            .get_item("f")
            .expect("function lookup should succeed")
            .expect("function should exist");
        ffi::Py_INCREF(function.as_ptr());
        function.as_ptr()
    }

    unsafe fn class_dict_function(
        owner_type: *mut ffi::PyTypeObject,
        name: &'static std::ffi::CStr,
    ) -> *mut ffi::PyObject {
        let dict = (*owner_type).tp_dict;
        assert!(!dict.is_null(), "owner type should have a tp_dict");
        let function = ffi::PyDict_GetItemString(dict, name.as_ptr());
        assert!(
            !function.is_null(),
            "class dict should contain requested function"
        );
        ffi::Py_INCREF(function);
        function
    }

    #[test]
    fn exact_owner_type_lookup_uses_current_class_dict_binding() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module(py);
            register_function_owner_types_for_module(module.as_ptr())
                .expect("owner type registration should succeed");

            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let function = class_dict_function(owner_type, c"f");

            let owners = lookup_exact_owner_types_for_function_object(function, "f", &{
                let registry =
                    function_owner_type_registry().expect("owner type registry should initialize");
                let registered = registry
                    .registered_owner_types_by_function
                    .lock()
                    .expect("owner type registry lock should succeed");
                registered
                    .get(&(function as usize))
                    .expect("registered owner types should contain class method")
                    .owner_type_weakrefs
                    .clone()
            })
            .expect("owner type lookup should succeed");
            assert_eq!(owners.len(), 1, "expected one exact owner type for C.f");
            assert_eq!(owners[0].owner_type, owner_type);
            assert_eq!(owners[0].function_obj, function);
            assert_ne!(owners[0].type_version, 0);

            let wrong_name = lookup_exact_owner_types_for_function_object(function, "g", &{
                let registry =
                    function_owner_type_registry().expect("owner type registry should initialize");
                let registered = registry
                    .registered_owner_types_by_function
                    .lock()
                    .expect("owner type registry lock should succeed");
                registered
                    .get(&(function as usize))
                    .expect("registered owner types should contain class method")
                    .owner_type_weakrefs
                    .clone()
            })
            .expect("owner type lookup with wrong name should still succeed");
            assert!(
                wrong_name.is_empty(),
                "wrong method name should not produce an exact owner binding"
            );

            ffi::Py_DECREF(function);
        });
    }

    #[test]
    fn pyfunction_soac_metadata_roundtrips_without_func_dict_storage() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            TEST_SOAC_METADATA_DROPS.store(0, Ordering::SeqCst);
            let function = make_test_function(py);
            let function_id = RuntimeFunctionId::from_raw_parts(7, 11);
            let metadata = Box::into_raw(Box::new(123usize)) as *mut c_void;
            assert_eq!(
                PyFunction_SetSoacMetadata(
                    function,
                    function_id.to_packed_runtime_u64(),
                    metadata,
                    Some(free_test_soac_metadata),
                ),
                0,
                "setting SOAC metadata should succeed"
            );
            assert_eq!(
                PyFunction_GetSoacMetadata(function),
                metadata,
                "metadata pointer should round-trip"
            );
            assert_eq!(
                PyFunction_GetSoacFunctionId(function),
                function_id.to_packed_runtime_u64(),
                "packed function id should round-trip"
            );
            assert_eq!(
                registered_clif_function_id(function).expect("function id lookup should succeed"),
                Some(function_id),
                "registered function id should decode from SOAC metadata"
            );
            assert_eq!(
                PyFunction_SetSoacMetadata(function, 0, ptr::null_mut(), None,),
                0,
                "clearing SOAC metadata should succeed"
            );
            assert!(
                PyFunction_GetSoacMetadata(function).is_null(),
                "cleared SOAC metadata should not retain a pointer"
            );
            assert_eq!(
                PyFunction_GetSoacFunctionId(function),
                0,
                "cleared SOAC function id should be unset"
            );
            assert_eq!(
                TEST_SOAC_METADATA_DROPS.load(Ordering::SeqCst),
                1,
                "clearing should invoke the registered metadata destructor once"
            );
            ffi::Py_DECREF(function);
        });
    }

    #[test]
    fn pyheaptype_soac_metadata_roundtrips_without_type_dict_storage() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            TEST_SOAC_METADATA_DROPS.store(0, Ordering::SeqCst);
            let (_module, cls) = make_test_module(py);
            let class_obj = cls.as_ptr();
            let function_id = RuntimeFunctionId::from_raw_parts(9, 3);
            let metadata = Box::into_raw(Box::new(321usize)) as *mut c_void;
            assert_eq!(
                PyType_SetSoacMetadata(
                    class_obj,
                    function_id.to_packed_runtime_u64(),
                    metadata,
                    Some(free_test_soac_metadata),
                ),
                0,
                "setting SOAC type metadata should succeed"
            );
            assert_eq!(
                PyType_GetSoacMetadata(class_obj),
                metadata,
                "type metadata pointer should round-trip"
            );
            assert_eq!(
                PyType_GetSoacFunctionId(class_obj),
                function_id.to_packed_runtime_u64(),
                "packed type function id should round-trip"
            );
            assert_eq!(
                registered_clif_type_function_id(class_obj)
                    .expect("type function id lookup should succeed"),
                Some(function_id),
                "registered type function id should decode from SOAC metadata"
            );
            assert_eq!(
                PyType_SetSoacMetadata(class_obj, 0, ptr::null_mut(), None,),
                0,
                "clearing SOAC type metadata should succeed"
            );
            assert!(
                PyType_GetSoacMetadata(class_obj).is_null(),
                "cleared SOAC type metadata should not retain a pointer"
            );
            assert_eq!(
                PyType_GetSoacFunctionId(class_obj),
                0,
                "cleared SOAC type function id should be unset"
            );
            assert_eq!(
                TEST_SOAC_METADATA_DROPS.load(Ordering::SeqCst),
                1,
                "clearing type metadata should invoke the registered destructor once"
            );
        });
    }

    #[test]
    fn owner_type_registration_sets_constructor_type_function_id() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module(py);
            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let init_function = class_dict_function(owner_type, c"__init__");
            let function_id = RuntimeFunctionId::from_raw_parts(10, 4);
            assert_eq!(
                PyFunction_SetSoacMetadata(
                    init_function,
                    function_id.to_packed_runtime_u64(),
                    ptr::null_mut(),
                    None,
                ),
                0,
                "registering __init__ SOAC id should succeed"
            );
            register_function_owner_types_for_module(module.as_ptr())
                .expect("owner type registration should succeed");
            assert_eq!(
                PyType_GetSoacFunctionId(cls.as_ptr()),
                function_id.to_packed_runtime_u64(),
                "owner type registration should attach packed __init__ function id"
            );
            assert_eq!(
                registered_clif_type_function_id(cls.as_ptr())
                    .expect("type function id lookup should succeed"),
                Some(function_id),
                "owner type registration should decode the attached constructor id"
            );
            ffi::Py_DECREF(init_function);
        });
    }

    #[test]
    fn owner_type_registration_module_keys_do_not_invoke_module_getattr() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let module = PyModule::new(py, "watcher_test").expect("module should allocate");
            let globals = module.dict();
            let calls = PyList::empty(py);
            let builtins = py.import("builtins").expect("builtins should import");
            globals
                .set_item("__builtins__", &builtins)
                .expect("module globals should accept builtins");
            globals
                .set_item("calls", &calls)
                .expect("module globals should accept calls list");

            let source = std::ffi::CString::new(
                "def __getattr__(name):\n    calls.append(name)\n    raise AttributeError(name)\n",
            )
            .expect("python source should be CString-compatible");
            assert!(
                !ffi::PyRun_StringFlags(
                    source.as_ptr(),
                    ffi::Py_file_input,
                    globals.as_ptr(),
                    globals.as_ptr(),
                    ptr::null_mut(),
                )
                .is_null(),
                "module getattr definition should execute"
            );

            register_function_owner_types_for_module_keys(module.as_ptr(), &["C".to_string()])
                .expect("indexed module-key registration should succeed");
            assert_eq!(
                calls.len(),
                0,
                "registration should read indexed dict storage directly, not module __getattr__"
            );
        });
    }

    #[test]
    fn owner_type_registration_ignores_static_module_types() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::new(py, "typing").expect("module should allocate");
            let typing = py.import("typing").expect("typing should import");
            module
                .dict()
                .set_item(
                    "Union",
                    typing
                        .getattr("Union")
                        .expect("typing should expose static Union type"),
                )
                .expect("module globals should accept a static type");

            unsafe {
                register_function_owner_types_for_module(module.as_ptr())
                    .expect("owner type registration should skip unsupported static types");
            }
        });
    }

    #[test]
    fn exact_constructor_owner_lookup_returns_simple_heap_types() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module(py);
            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let init_function = class_dict_function(owner_type, c"__init__");
            let function_id = RuntimeFunctionId::from_raw_parts(10, 7);
            assert_eq!(
                PyFunction_SetSoacMetadata(
                    init_function,
                    function_id.to_packed_runtime_u64(),
                    ptr::null_mut(),
                    None,
                ),
                0,
                "registering __init__ SOAC id should succeed"
            );
            register_function_owner_types_for_module(module.as_ptr())
                .expect("owner type registration should succeed");
            let owners = lookup_exact_owner_types_for_constructor(function_id)
                .expect("constructor owner lookup should succeed");
            assert_eq!(owners.len(), 1, "expected one constructor owner type");
            assert_eq!(owners[0].owner_type, owner_type);
            assert_eq!(owners[0].init_function_obj, init_function);
            assert_ne!(owners[0].type_version, 0);
            ffi::Py_DECREF(init_function);
        });
    }

    #[test]
    fn exact_constructor_owner_lookup_skips_custom_new_types() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module_with_source(
                py,
                "class C:\n    def __new__(cls, value):\n        return super().__new__(cls)\n    def __init__(self, value):\n        self.value = value\n",
            );
            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let init_function = class_dict_function(owner_type, c"__init__");
            let function_id = RuntimeFunctionId::from_raw_parts(10, 8);
            assert_eq!(
                PyFunction_SetSoacMetadata(
                    init_function,
                    function_id.to_packed_runtime_u64(),
                    ptr::null_mut(),
                    None,
                ),
                0,
                "registering __init__ SOAC id should succeed"
            );
            register_function_owner_types_for_module(module.as_ptr())
                .expect("owner type registration should succeed");
            let owners = lookup_exact_owner_types_for_constructor(function_id)
                .expect("constructor owner lookup should succeed");
            assert!(
                owners.is_empty(),
                "custom __new__ types should not use the simple constructor fast path"
            );
            ffi::Py_DECREF(init_function);
        });
    }

    #[test]
    fn function_watcher_invalidates_owner_type_version_for_all_expected_mutations() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module(py);
            register_function_owner_types_for_module(module.as_ptr())
                .expect("owner type registration should succeed");

            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let func = ffi::PyObject_GetAttrString(cls.as_ptr(), c"f".as_ptr());
            assert!(
                !func.is_null(),
                "class attribute lookup should resolve function"
            );

            assert_function_mutation_invalidates_type_version(owner_type, "defaults", || {
                let defaults = ffi::PyTuple_New(1);
                assert!(!defaults.is_null(), "defaults tuple should allocate");
                let default_value = ffi::PyLong_FromLongLong(3);
                assert!(!default_value.is_null(), "default value should allocate");
                assert_eq!(ffi::PyTuple_SetItem(defaults, 0, default_value), 0);
                let result = PyFunction_SetDefaults(func, defaults);
                ffi::Py_DECREF(defaults);
                result
            });

            assert_function_mutation_invalidates_type_version(owner_type, "kwdefaults", || {
                let kwdefaults = ffi::PyDict_New();
                assert!(!kwdefaults.is_null(), "kwdefaults dict should allocate");
                let key = ffi::PyUnicode_FromString(c"y".as_ptr());
                let value = ffi::PyLong_FromLongLong(4);
                assert!(!key.is_null(), "kwdefaults key should allocate");
                assert!(!value.is_null(), "kwdefaults value should allocate");
                assert_eq!(ffi::PyDict_SetItem(kwdefaults, key, value), 0);
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(value);
                let result = PyFunction_SetKwDefaults(func, kwdefaults);
                ffi::Py_DECREF(kwdefaults);
                result
            });

            assert_function_mutation_invalidates_type_version(owner_type, "qualname", || {
                let qualname = ffi::PyUnicode_FromString(c"C.f_renamed".as_ptr());
                assert!(!qualname.is_null(), "qualname should allocate");
                let result = ffi::PyObject_SetAttrString(func, c"__qualname__".as_ptr(), qualname);
                ffi::Py_DECREF(qualname);
                result
            });

            assert_function_mutation_invalidates_type_version(owner_type, "code", || {
                let replacement_code = compile_replacement_function(py);
                let result =
                    ffi::PyObject_SetAttrString(func, c"__code__".as_ptr(), replacement_code);
                ffi::Py_DECREF(replacement_code);
                result
            });

            ffi::Py_DECREF(func);
        });
    }
}
