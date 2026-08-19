#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_unsafe)]

include!(concat!(env!("OUT_DIR"), "/soac_jit_runtime_clif.rs"));

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

pub mod config;
pub mod function_instantiation;
pub mod import_helpers;
mod jit;
pub use function_instantiation::{
    instantiate_bb_function, make_function, make_function_from_python_args,
};
pub use jit::*;

pub mod counter;
pub mod module_constants;
pub mod module_type;
pub mod preserved_state;
pub mod session;

pub use session::{CompileSession, CompileSessionId, allocate_compile_session_id};

#[cfg(test)]
pub(crate) fn python_runtime_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

use pyo3::ffi;
use pyo3::prelude::*;
use soac_core::block_py::{
    BlockPyFunction, ClosureInit, FunctionExecutionMode, FunctionKind, ParamKind,
    PreservedSlotStorage, RuntimeFunctionId, RuntimeName,
};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::plan_v3::{IndexedFieldAccessKind, LateBoundOwnerFieldStorage};
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::any::Any;
use std::ffi::{CString, c_char, c_void};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Instant;
use tracing::info;

#[cfg(test)]
pub(crate) fn test_repo_root() -> std::path::PathBuf {
    soac_cpython::repo_root()
}

#[cfg(test)]
pub(crate) fn initialize_test_python() {
    soac_cpython::initialize_test_python("soac_jit-test").expect("test Python should initialize");
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
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
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

unsafe fn function_may_have_registered_owner_types(function: *mut ffi::PyFunctionObject) -> bool {
    unsafe { !(*function).func_weakreflist.is_null() }
}

unsafe extern "C" fn function_owner_type_watcher_callback(
    event: PyFunctionWatchEvent,
    func: *mut ffi::PyFunctionObject,
    new_value: *mut ffi::PyObject,
) -> i32 {
    if event == PY_FUNCTION_EVENT_CREATE
        || (event == PY_FUNCTION_EVENT_DESTROY
            && !unsafe { function_may_have_registered_owner_types(func) })
    {
        return 0;
    }
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
                    unsafe { jit::invalidate_py_function_soac_function_id(func) };
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
            if !unsafe { function_may_have_registered_owner_types(func) } {
                return 0;
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
            if !unsafe { function_may_have_registered_owner_types(func) } {
                return 0;
            }
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
    match FUNCTION_OWNER_TYPE_REGISTRY.get_or_init(|| {
        Ok(FunctionOwnerTypeRegistry {
            watcher_id: AtomicI32::new(-1),
            registered_owner_types_by_function: Mutex::new(HashMap::new()),
        })
    }) {
        Ok(registry) => Ok(registry),
        Err(()) => Err(()),
    }
}

fn ensure_function_owner_type_watcher(registry: &FunctionOwnerTypeRegistry) -> Result<(), ()> {
    if registry.watcher_id.load(Ordering::Acquire) >= 0 {
        return Ok(());
    }

    // Function-owner registration and CPython watcher installation both run
    // while the interpreter lock is held.
    let watcher_id = unsafe { PyFunction_AddWatcher(function_owner_type_watcher_callback) };
    if watcher_id < 0 {
        return Err(());
    }
    registry.watcher_id.store(watcher_id, Ordering::Release);
    Ok(())
}

fn set_runtime_error<T>(msg: &str) -> Result<T, ()> {
    unsafe {
        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, CString::new(msg).unwrap().as_ptr());
    }
    Err(())
}

fn set_runtime_error_message(msg: &str) {
    let _ = set_runtime_error::<()>(msg);
}

#[repr(C)]
struct FunctionEnvAbiHeader {
    direct_code_ptr: *const u8,
    default_direct_code_ptr: *const u8,
    deopt_table_ptr: *const c_void,
    globals_obj: *mut ffi::PyObject,
    builtins_obj: *mut ffi::PyObject,
    late_bound_owner_cells: *const module_type::LateBoundOwnerFieldCell,
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
    function_template: Arc<FunctionInstantiationTemplate>,
    compile_session: Arc<CompileSession>,
    module_state: Arc<module_type::SharedModuleState>,
    compiled_vectorcall_entry: Option<jit::VectorcallEntryFn>,
    previous_vectorcall: Option<ffi::vectorcallfunc>,
    registered_code: *mut ffi::PyObject,
    registered_defaults: *mut ffi::PyObject,
    registered_kwdefaults: *mut ffi::PyObject,
}

pub(crate) const PY_FUNCTION_JIT_EXTRA_REGISTERED_CODE_OFFSET: i32 =
    mem::offset_of!(PyFunctionJitExtra, registered_code) as i32;
pub(crate) const PY_FUNCTION_JIT_EXTRA_REGISTERED_DEFAULTS_OFFSET: i32 =
    mem::offset_of!(PyFunctionJitExtra, registered_defaults) as i32;
pub(crate) const PY_FUNCTION_JIT_EXTRA_REGISTERED_KWDEFAULTS_OFFSET: i32 =
    mem::offset_of!(PyFunctionJitExtra, registered_kwdefaults) as i32;

impl Drop for PyFunctionJitExtra {
    fn drop(&mut self) {
        unsafe {
            if !self.registered_code.is_null() {
                ffi::Py_DECREF(self.registered_code);
            }
            if !self.registered_defaults.is_null() {
                ffi::Py_DECREF(self.registered_defaults);
            }
            if !self.registered_kwdefaults.is_null() {
                ffi::Py_DECREF(self.registered_kwdefaults);
            }
        }
    }
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
    fn from_function(function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>) -> Self {
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

    fn binds_exact_positional(&self, nargsf: usize, kwnames: *mut ffi::PyObject) -> bool {
        kwnames.is_null()
            && self.varargs_param.is_none()
            && self.varkw_param.is_none()
            && self.positional_capacity() == self.param_count()
            && unsafe { ffi::PyVectorcall_NARGS(nargsf) as usize } == self.param_count()
    }

    fn param_index(&self, name: &str) -> Option<usize> {
        self.param_indices_by_name.get(name).copied()
    }
}

pub(crate) struct FunctionInstantiationTemplate {
    function: Arc<BlockPyFunction<BlockPyModuleShape>>,
    capture_names: Box<[String]>,
    runtime_data_layout: jit::FunctionRuntimeDataLayout,
    binding_plan: DirectArgBindingPlan,
    entry_plan: jit::RuntimeFunctionEntryPlan,
    prepared_original_code: OnceLock<Option<function_instantiation::PreparedOriginalCode>>,
    prepared_synthetic_code: OnceLock<function_instantiation::PreparedSyntheticCode>,
    prepared_runtime_lookup_keys: OnceLock<function_instantiation::PreparedRuntimeLookupKeys>,
    prepared_bootstrap_factory_origin:
        OnceLock<function_instantiation::PreparedBootstrapFactoryOrigin>,
    prepared_eager_comprehension: OnceLock<function_instantiation::PreparedEagerComprehension>,
    prepared_direct_entry: OnceLock<PreparedDirectEntry>,
    prepared_vectorcall_trampoline: OnceLock<PreparedVectorcallTrampoline>,
    prepared_generator_factory: OnceLock<PreparedGeneratorFactory>,
    prepared_stop_iteration_matcher: OnceLock<PreparedStopIterationMatcher>,
}

#[derive(Clone, Copy)]
struct PreparedStopIterationDictionaryEntry {
    index: usize,
    key: usize,
    value: usize,
}

struct PreparedStopIterationMatcher {
    compile_session_id: CompileSessionId,
    helper_function_id: RuntimeFunctionId,
    helper: usize,
    helper_code: usize,
    validator_function_id: RuntimeFunctionId,
    validator: usize,
    validator_code: usize,
    runtime_globals: usize,
    runtime_keys: usize,
    runtime_values: usize,
    builtins: usize,
    builtin_keys: usize,
    runtime_entries: [PreparedStopIterationDictionaryEntry; 7],
    builtin_entries: [PreparedStopIterationDictionaryEntry; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedDirectEntryKey {
    compile_session_id: CompileSessionId,
    code_ptr: usize,
    code_version: u32,
}

struct PreparedDirectEntry {
    key: PreparedDirectEntryKey,
    handle: Arc<jit::CompiledFunctionHandle>,
}

#[derive(Clone, Copy)]
struct PreparedVectorcallTrampoline {
    compile_session_id: CompileSessionId,
    param_count: usize,
    entry: jit::VectorcallEntryFn,
}

impl PreparedVectorcallTrampoline {
    fn matches(&self, compile_session_id: CompileSessionId, param_count: usize) -> bool {
        self.compile_session_id == compile_session_id && self.param_count == param_count
    }
}

struct PreparedGeneratorFactory {
    compile_session_id: CompileSessionId,
    helper_function_id: RuntimeFunctionId,
    helper: usize,
    helper_code: usize,
    runtime_globals: usize,
    builtin_getattr: usize,
    preserved_state_factory: usize,
    generator_class: usize,
    generator_class_version: u32,
    code_template: usize,
    getattr_key: Py<PyAny>,
    preserved_state_key: Py<PyAny>,
    generator_class_key: Py<PyAny>,
    code_template_key: Py<PyAny>,
}

impl FunctionInstantiationTemplate {
    pub(crate) fn from_function(
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<Self, String> {
        let capture_names = function
            .public_storage_layout()
            .map(|layout| {
                layout
                    .freevars
                    .iter()
                    .map(|slot| slot.logical_name.clone())
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            })
            .unwrap_or_default();
        let runtime_data_layout = jit::FunctionRuntimeDataLayout::from_function(function);
        let binding_plan = DirectArgBindingPlan::from_function(function);
        let entry_plan = jit::RuntimeFunctionEntryPlan::from_function(function)?;
        Ok(Self {
            function: Arc::new(function.clone()),
            capture_names,
            runtime_data_layout,
            binding_plan,
            entry_plan,
            prepared_original_code: OnceLock::new(),
            prepared_synthetic_code: OnceLock::new(),
            prepared_runtime_lookup_keys: OnceLock::new(),
            prepared_bootstrap_factory_origin: OnceLock::new(),
            prepared_eager_comprehension: OnceLock::new(),
            prepared_direct_entry: OnceLock::new(),
            prepared_vectorcall_trampoline: OnceLock::new(),
            prepared_generator_factory: OnceLock::new(),
            prepared_stop_iteration_matcher: OnceLock::new(),
        })
    }

    pub(crate) fn function(&self) -> &BlockPyFunction<BlockPyModuleShape> {
        self.function.as_ref()
    }

    pub(crate) fn capture_names(&self) -> &[String] {
        &self.capture_names
    }

    fn runtime_data_layout(&self) -> &jit::FunctionRuntimeDataLayout {
        &self.runtime_data_layout
    }

    fn binding_plan(&self) -> &DirectArgBindingPlan {
        &self.binding_plan
    }

    fn entry_plan(&self) -> &jit::RuntimeFunctionEntryPlan {
        &self.entry_plan
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
        builtins_obj: *mut ffi::PyObject,
        late_bound_owner_cells: *const module_type::LateBoundOwnerFieldCell,
        mut runtime_object_values: Box<[*mut ffi::PyObject]>,
    ) -> Result<Self, ()> {
        if globals_obj.is_null() || builtins_obj.is_null() {
            unsafe { cleanup_state_values(&mut runtime_object_values) };
            return set_runtime_error(
                "missing globals or captured builtins while creating JIT function environment",
            );
        }
        unsafe {
            ffi::Py_INCREF(globals_obj);
            ffi::Py_INCREF(builtins_obj);
        }
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
                builtins_obj,
                late_bound_owner_cells,
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

    fn builtins_obj(&self) -> *mut ffi::PyObject {
        self.header().builtins_obj
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
        let builtins_obj = self.builtins_obj();
        if !builtins_obj.is_null() {
            unsafe { ffi::Py_DECREF(builtins_obj) };
            self.header_mut().builtins_obj = ptr::null_mut();
        }
        let layout = Self::allocation_layout(self.runtime_object_len);
        unsafe { dealloc(self.abi.as_ptr() as *mut u8, layout) };
    }
}

impl PyFunctionJitExtra {
    fn function(&self) -> Result<&soac_core::block_py::BlockPyFunction<BlockPyModuleShape>, ()> {
        Ok(self.function_template.function())
    }

    unsafe fn refresh_runtime_objects_after_function_update(
        &mut self,
        callable: *mut ffi::PyObject,
        event: PyFunctionWatchEvent,
        new_value: *mut ffi::PyObject,
    ) -> Result<(), ()> {
        let defaults_override = (event == PY_FUNCTION_EVENT_MODIFY_DEFAULTS).then_some(new_value);
        let kwdefaults_override =
            (event == PY_FUNCTION_EVENT_MODIFY_KWDEFAULTS).then_some(new_value);
        let values = unsafe {
            collect_function_runtime_objects(
                callable,
                self.function_template.runtime_data_layout(),
                defaults_override,
                kwdefaults_override,
            )?
        };
        unsafe { self.function_env.replace_runtime_objects(values)? };
        if event == PY_FUNCTION_EVENT_MODIFY_DEFAULTS {
            unsafe { replace_owned_function_snapshot(&mut self.registered_defaults, new_value) };
        }
        if event == PY_FUNCTION_EVENT_MODIFY_KWDEFAULTS {
            unsafe { replace_owned_function_snapshot(&mut self.registered_kwdefaults, new_value) };
        }
        Ok(())
    }

    unsafe fn refresh_runtime_objects_from_current_function(
        &mut self,
        callable: *mut ffi::PyObject,
    ) -> Result<(), ()> {
        let function = callable.cast::<ffi::PyFunctionObject>();
        let current_defaults = unsafe { (*function).func_defaults };
        let current_kwdefaults = unsafe { (*function).func_kwdefaults };
        let has_mutable_kwdefaults = !current_kwdefaults.is_null()
            && self
                .function_template
                .runtime_data_layout()
                .kwonly_default_slots()
                .next()
                .is_some();
        if current_defaults == self.registered_defaults
            && current_kwdefaults == self.registered_kwdefaults
            && !has_mutable_kwdefaults
        {
            return Ok(());
        }

        let values = unsafe {
            collect_function_runtime_objects(
                callable,
                self.function_template.runtime_data_layout(),
                None,
                None,
            )?
        };
        unsafe {
            self.function_env.replace_runtime_objects(values)?;
            replace_owned_function_snapshot(&mut self.registered_defaults, current_defaults);
            replace_owned_function_snapshot(&mut self.registered_kwdefaults, current_kwdefaults);
        }
        Ok(())
    }
}

unsafe fn replace_owned_function_snapshot(
    snapshot: &mut *mut ffi::PyObject,
    replacement: *mut ffi::PyObject,
) {
    if *snapshot == replacement {
        return;
    }
    if !replacement.is_null() {
        unsafe { ffi::Py_INCREF(replacement) };
    }
    let previous = mem::replace(snapshot, replacement);
    if !previous.is_null() {
        unsafe { ffi::Py_DECREF(previous) };
    }
}

#[derive(Clone, Debug)]
struct RegisteredFunctionOwnerTypes {
    function_weakref: usize,
    owner_type_weakrefs: Vec<usize>,
}

struct FunctionOwnerTypeRegistry {
    watcher_id: AtomicI32,
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
        let _ = self.watcher_id.load(Ordering::Relaxed);
    }
}

static FUNCTION_OWNER_TYPE_REGISTRY: OnceLock<Result<FunctionOwnerTypeRegistry, ()>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub(crate) struct FunctionOwnerType {
    pub function_obj: *mut ffi::PyObject,
    pub owner_type: *mut ffi::PyTypeObject,
    pub type_version: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ConstructorOwnerType {
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

pub unsafe fn eager_compile_jit_for_module(module: *mut ffi::PyObject) -> Result<(), ()> {
    let py = Python::assume_attached();
    if module.is_null() {
        return set_runtime_error("missing transformed module for eager JIT compile");
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
                    b"failed to initialize process JIT for eager compile\0".as_ptr()
                        as *const c_char,
                )
            };
        }
    })?;
    unsafe { process_jit.eager_compile_shared_module(Arc::clone(&compile_session), shared_state) }
        .map_err(|err| {
            if let Ok(c_msg) = CString::new(err) {
                unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr()) };
            } else {
                unsafe {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to eagerly compile transformed module\0".as_ptr() as *const c_char,
                    )
                };
            }
        })
}

unsafe fn make_clif_function_data(
    callable: *mut ffi::PyObject,
    function_id: RuntimeFunctionId,
    module_runtime: jit::ModuleRuntimeContext,
    known_function_template: Option<Arc<FunctionInstantiationTemplate>>,
) -> Result<*mut c_void, ()> {
    let module_state = module_runtime.shared_module_state_owner.clone();
    let function_template = match known_function_template {
        Some(template) => {
            if template.function().function_id != function_id {
                return set_runtime_error(
                    "known function template does not match its CLIF function identifier",
                );
            }
            Some(template)
        }
        None => module_state
            .lookup_function_template(function_id)
            .map_err(|err| set_runtime_error_message(&err))?,
    };
    let Some(function_template) = function_template else {
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
    let runtime_object_values = unsafe {
        collect_function_runtime_objects(
            callable,
            function_template.runtime_data_layout(),
            None,
            None,
        )?
    };
    let raw_function = callable.cast::<ffi::PyFunctionObject>();
    let mut function_env = unsafe {
        Box::new(FunctionEnv::new(
            module_runtime.mod_ctx.globals_obj as *mut ffi::PyObject,
            (*raw_function).func_builtins,
            module_runtime
                .shared_module_state_owner
                .late_bound_owner_fields
                .cells
                .as_ptr(),
            runtime_object_values,
        )?)
    };
    let function_env_ptr = function_env.as_mut_ptr();
    let registered_code = unsafe { (*raw_function).func_code };
    let registered_defaults = unsafe { (*raw_function).func_defaults };
    let registered_kwdefaults = unsafe { (*raw_function).func_kwdefaults };
    unsafe {
        ffi::Py_INCREF(registered_code);
        if !registered_defaults.is_null() {
            ffi::Py_INCREF(registered_defaults);
        }
        if !registered_kwdefaults.is_null() {
            ffi::Py_INCREF(registered_kwdefaults);
        }
    }
    let py_function_extra = Box::new(PyFunctionJitExtra {
        function_env_ptr,
        function_id,
        function_env,
        function_template,
        compile_session: module_runtime.compile_session.clone(),
        module_state,
        compiled_vectorcall_entry: None,
        previous_vectorcall: unsafe { (*(callable as *mut ffi::PyFunctionObject)).vectorcall },
        registered_code,
        registered_defaults,
        registered_kwdefaults,
    });
    Ok(Box::into_raw(py_function_extra) as *mut c_void)
}

unsafe fn py_function_jit_extra(
    function: *mut ffi::PyObject,
) -> Result<&'static mut PyFunctionJitExtra, ()> {
    if ffi::PyFunction_Check(function) == 0 {
        ffi::PyErr_SetString(
            ffi::PyExc_TypeError,
            b"expected Python function for CLIF vectorcall data lookup\0"
                .as_ptr()
                .cast(),
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
    watch_owner_mutations: bool,
) -> Result<(), ()> {
    let registry = function_owner_type_registry()?;
    if watch_owner_mutations {
        ensure_function_owner_type_watcher(registry)?;
    }
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
) -> Result<Vec<FunctionOwnerType>, ()> {
    let method_name = CString::new(method_name).map_err(|_| {
        ffi::PyErr_SetString(
            ffi::PyExc_ValueError,
            c"function owner lookup name contained NUL".as_ptr(),
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
                    out.push(FunctionOwnerType {
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

unsafe fn lookup_exact_owner_types_for_registered_function(
    function_id: RuntimeFunctionId,
    method_name: &str,
) -> Result<Vec<FunctionOwnerType>, ()> {
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

pub(crate) unsafe fn lookup_exact_owner_types_for_method(
    function_id: RuntimeFunctionId,
    method_name: &str,
) -> Result<Vec<FunctionOwnerType>, ()> {
    lookup_exact_owner_types_for_registered_function(function_id, method_name)
}

pub unsafe fn lookup_exact_owner_types_for_constructor(
    function_id: RuntimeFunctionId,
) -> Result<Vec<ConstructorOwnerType>, ()> {
    let owners = lookup_exact_owner_types_for_registered_function(function_id, "__init__")?;
    let mut out = Vec::new();
    for owner in owners {
        out.push(ConstructorOwnerType {
            init_function_obj: owner.function_obj,
            owner_type: owner.owner_type,
            type_version: owner.type_version,
        });
    }
    out.sort_by_key(|entry| (entry.owner_type as usize, entry.init_function_obj as usize));
    out.dedup_by_key(|entry| (entry.owner_type as usize, entry.init_function_obj as usize));
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

unsafe fn owner_type_supports_direct_constructor_entry(owner_type: *mut ffi::PyTypeObject) -> bool {
    if owner_type.is_null() {
        return false;
    }
    if ((*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE) == 0
        || ((*owner_type).tp_flags & ffi::Py_TPFLAGS_IS_ABSTRACT) != 0
    {
        return false;
    }
    if ffi::Py_TYPE(owner_type as *mut ffi::PyObject) != ptr::addr_of_mut!(ffi::PyType_Type) {
        return false;
    }
    let Some(owner_tp_alloc) = (*owner_type).tp_alloc else {
        return false;
    };
    let generic_alloc: unsafe extern "C" fn(
        *mut ffi::PyTypeObject,
        ffi::Py_ssize_t,
    ) -> *mut ffi::PyObject = ffi::PyType_GenericAlloc;
    if !ptr::fn_addr_eq(owner_tp_alloc, generic_alloc) {
        return false;
    }
    let Some(owner_tp_new) = (*owner_type).tp_new else {
        return false;
    };
    let Some(base_object_tp_new) = ffi::PyBaseObject_Type.tp_new else {
        return false;
    };
    ptr::fn_addr_eq(owner_tp_new, base_object_tp_new)
}

unsafe fn owner_type_has_generic_attribute_hooks(owner_type: *mut ffi::PyTypeObject) -> bool {
    let has_generic_getattr = (*owner_type).tp_getattro.is_some_and(|getattr| {
        ptr::fn_addr_eq(
            getattr,
            ffi::PyObject_GenericGetAttr
                as unsafe extern "C" fn(
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                ) -> *mut ffi::PyObject,
        )
    });
    let has_generic_setattr = (*owner_type).tp_setattro.is_some_and(|setattr| {
        ptr::fn_addr_eq(
            setattr,
            ffi::PyObject_GenericSetAttr
                as unsafe extern "C" fn(
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                ) -> i32,
        )
    });
    has_generic_getattr && has_generic_setattr
}

unsafe fn owner_type_has_any_class_binding(
    owner_type: *mut ffi::PyTypeObject,
    attr_name: &CString,
) -> bool {
    let mro = (*owner_type).tp_mro;
    if mro.is_null() || ffi::PyTuple_CheckExact(mro) == 0 {
        return true;
    }
    for index in 0..ffi::PyTuple_GET_SIZE(mro) {
        let base = ffi::PyTuple_GET_ITEM(mro, index).cast::<ffi::PyTypeObject>();
        if base.is_null() {
            return true;
        }
        // Static builtins keep their per-interpreter dictionaries outside
        // PyTypeObject, so tp_dict is not a valid general MRO dictionary accessor.
        let dict = ffi::PyType_GetDict(base);
        if dict.is_null() {
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
            }
            return true;
        }
        let found = !ffi::PyDict_GetItemString(dict, attr_name.as_ptr()).is_null();
        let failed = !ffi::PyErr_Occurred().is_null();
        ffi::Py_DECREF(dict);
        if found || failed {
            ffi::PyErr_Clear();
            return true;
        }
    }
    false
}

unsafe fn late_bound_slot_offset_for_owner(
    owner_type: *mut ffi::PyTypeObject,
    attr_name: &CString,
    access: IndexedFieldAccessKind,
) -> Option<usize> {
    if (*owner_type).tp_itemsize != 0 || (*owner_type).tp_dict.is_null() {
        return None;
    }
    let descriptor = ffi::PyDict_GetItemString((*owner_type).tp_dict, attr_name.as_ptr());
    if descriptor.is_null()
        || ffi::Py_TYPE(descriptor) != ptr::addr_of_mut!(ffi::PyMemberDescr_Type)
    {
        if !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
        }
        return None;
    }
    let descriptor = descriptor.cast::<ffi::PyMemberDescrObject>();
    if (*descriptor).d_common.d_type != owner_type {
        return None;
    }
    let member = (*descriptor).d_member;
    if member.is_null() || (*member).name.is_null() {
        return None;
    }
    if (*member).type_code != ffi::Py_T_OBJECT_EX {
        return None;
    }
    let flags = (*member).flags;
    let allowed_flags = if access == IndexedFieldAccessKind::Load {
        ffi::Py_READONLY
    } else {
        0
    };
    if (flags & !allowed_flags) != 0 {
        return None;
    }
    let offset = usize::try_from((*member).offset).ok()?;
    let basicsize = usize::try_from((*owner_type).tp_basicsize).ok()?;
    if offset == 0
        || offset % mem::align_of::<*mut ffi::PyObject>() != 0
        || offset.checked_add(mem::size_of::<*mut ffi::PyObject>())? > basicsize
    {
        return None;
    }
    Some(offset)
}

unsafe fn publish_late_bound_owner_fields_for_function(
    function: *mut ffi::PyObject,
    owner_type: *mut ffi::PyTypeObject,
    shared_state: &module_type::SharedModuleState,
) -> Result<(), ()> {
    if !owner_type_has_generic_attribute_hooks(owner_type) {
        return Ok(());
    }
    let metadata = PyFunction_GetSoacMetadata(function);
    if metadata.is_null() {
        return Ok(());
    }
    let metadata = &*(metadata as *const PyFunctionJitExtra);
    if !ptr::eq(Arc::as_ptr(&metadata.module_state), shared_state) {
        return Ok(());
    }
    let owner_qualname = ffi::PyType_GetQualName(owner_type);
    if owner_qualname.is_null() {
        return Err(());
    }
    let mut length = 0;
    let qualname_utf8 = ffi::PyUnicode_AsUTF8AndSize(owner_qualname, &mut length);
    if qualname_utf8.is_null() {
        ffi::Py_DECREF(owner_qualname);
        return Err(());
    }
    let qualname_bytes = std::slice::from_raw_parts(qualname_utf8.cast::<u8>(), length as usize);
    let result = (|| {
        for (function_id, site) in &shared_state.late_bound_owner_fields.sites {
            if *function_id != metadata.function_id
                || site.owner_type.qualname.as_bytes() != qualname_bytes
            {
                continue;
            }
            let Some(cell) = shared_state
                .late_bound_owner_fields
                .cells
                .get(site.cell_index as usize)
            else {
                continue;
            };
            if cell.owner_weakref.load(Ordering::Acquire) != 0 {
                continue;
            }
            let Ok(attr_name) = CString::new(site.attr_name.as_str()) else {
                continue;
            };
            let slot_offset = match site.storage {
                LateBoundOwnerFieldStorage::ObjectSlot => {
                    let Some(offset) =
                        late_bound_slot_offset_for_owner(owner_type, &attr_name, site.access)
                    else {
                        continue;
                    };
                    offset
                }
                LateBoundOwnerFieldStorage::SplitDict { .. } => {
                    if owner_type_has_any_class_binding(owner_type, &attr_name) {
                        continue;
                    }
                    0
                }
            };
            if (*owner_type).tp_version_tag == 0 {
                let _ = PyUnstable_Type_AssignVersionTag(owner_type);
            }
            let version = (*owner_type).tp_version_tag;
            if version == 0 {
                continue;
            }
            let weakref = PyWeakref_NewRef(owner_type.cast(), ptr::null_mut());
            if weakref.is_null() {
                return Err(());
            }
            let py = Python::assume_attached();
            let weakref_owner = Bound::<PyAny>::from_owned_ptr(py, weakref).unbind();
            let mut owners = shared_state
                .late_bound_owner_fields
                .owner_weakrefs
                .lock()
                .map_err(|_| {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"late-bound owner weakref registry lock poisoned".as_ptr(),
                    );
                })?;
            owners.push(weakref_owner);
            cell.slot_offset.store(slot_offset, Ordering::Release);
            cell.type_version.store(version as usize, Ordering::Release);
            cell.owner_weakref
                .store(weakref as usize, Ordering::Release);
        }
        Ok(())
    })();
    ffi::Py_DECREF(owner_qualname);
    result
}

unsafe fn publish_inherited_late_bound_owner_fields(
    owner_type: *mut ffi::PyTypeObject,
    shared_state: &module_type::SharedModuleState,
) -> Result<(), ()> {
    if !owner_type_has_generic_attribute_hooks(owner_type) {
        return Ok(());
    }
    let mro = (*owner_type).tp_mro;
    if mro.is_null() || ffi::PyTuple_CheckExact(mro) == 0 {
        return Ok(());
    }

    let owner_qualname = ffi::PyType_GetQualName(owner_type);
    if owner_qualname.is_null() {
        return Err(());
    }
    let mut length = 0;
    let utf8 = ffi::PyUnicode_AsUTF8AndSize(owner_qualname, &mut length);
    if utf8.is_null() {
        ffi::Py_DECREF(owner_qualname);
        return Err(());
    }
    let owner_name = std::slice::from_raw_parts(utf8.cast::<u8>(), length as usize);
    let function_ids = shared_state
        .late_bound_owner_fields
        .sites
        .iter()
        .filter(|(_, site)| {
            site.owner_type.qualname.as_bytes() == owner_name
                && matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
        })
        .map(|(function_id, _)| *function_id)
        .collect::<HashSet<_>>();
    ffi::Py_DECREF(owner_qualname);
    if function_ids.is_empty() {
        return Ok(());
    }

    let mut published_functions = HashSet::new();
    for index in 1..ffi::PyTuple_GET_SIZE(mro) {
        let base = ffi::PyTuple_GET_ITEM(mro, index).cast::<ffi::PyTypeObject>();
        if base.is_null() {
            continue;
        }
        let dict = ffi::PyType_GetDict(base);
        if dict.is_null() {
            return Err(());
        }
        let result = (|| {
            let mut position: ffi::Py_ssize_t = 0;
            let mut key = ptr::null_mut();
            let mut value = ptr::null_mut();
            while ffi::PyDict_Next(dict, &mut position, &mut key, &mut value) != 0 {
                if ffi::PyFunction_Check(value) == 0 || !published_functions.insert(value as usize)
                {
                    continue;
                }
                let metadata = PyFunction_GetSoacMetadata(value);
                if metadata.is_null() {
                    continue;
                }
                let metadata = &*(metadata as *const PyFunctionJitExtra);
                if !ptr::eq(Arc::as_ptr(&metadata.module_state), shared_state)
                    || !function_ids.contains(&metadata.function_id)
                {
                    continue;
                }
                publish_late_bound_owner_fields_for_function(value, owner_type, shared_state)?;
            }
            Ok(())
        })();
        ffi::Py_DECREF(dict);
        result?;
    }
    Ok(())
}

unsafe fn register_owner_types_from_type(
    owner_type: *mut ffi::PyTypeObject,
    module_name: *mut ffi::PyObject,
    visited_types: &mut HashSet<usize>,
    shared_state: Option<&module_type::SharedModuleState>,
    module_runtime: Option<&ModuleRuntimeContext>,
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
    let mut constructor_init_function = ptr::null_mut();
    let mut pos: ffi::Py_ssize_t = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while ffi::PyDict_Next(dict, &mut pos, &mut key, &mut value) != 0 {
        if ffi::PyFunction_Check(value) != 0 {
            if ffi::PyUnicode_Check(key) != 0
                && ffi::PyUnicode_CompareWithASCIIString(key, c"__init__".as_ptr()) == 0
            {
                if let Some(init_function_id) = registered_clif_function_id(value)? {
                    constructor_function_id = match shared_state {
                        Some(state) => soac_ir_blockpy::constructor_entry_function_id_for_init(
                            &state.lowered_module,
                            init_function_id,
                        ),
                        None => Some(init_function_id),
                    };
                    constructor_init_function = value;
                }
            }
            let compiler_owned_runtime =
                shared_state.is_some_and(|state| state.module_name == "soac.runtime");
            register_owner_type_for_function(value, owner_type, !compiler_owned_runtime)?;
            if let Some(shared_state) = shared_state {
                publish_late_bound_owner_fields_for_function(value, owner_type, shared_state)?;
            }
        } else if ffi::PyType_Check(value) != 0 {
            register_owner_types_from_type(
                value as *mut ffi::PyTypeObject,
                module_name,
                visited_types,
                shared_state,
                module_runtime,
            )?;
        }
    }
    if let Some(shared_state) = shared_state {
        publish_inherited_late_bound_owner_fields(owner_type, shared_state)?;
    }
    if let Some(function_id) = constructor_function_id
        && owner_type_supports_direct_constructor_entry(owner_type)
    {
        let mut metadata = ptr::null_mut();
        let mut metadata_destructor: Option<unsafe extern "C" fn(*mut c_void)> = None;
        if let Some(module_runtime) = module_runtime {
            let owned_runtime = clone_module_runtime_context(module_runtime)?;
            metadata = make_clif_function_data(
                constructor_init_function,
                function_id,
                owned_runtime,
                None,
            )?;
            metadata_destructor = Some(free_clif_function_data);
        }
        if PyType_SetSoacMetadata(
            owner_type as *mut ffi::PyObject,
            function_id.to_packed_runtime_u64(),
            metadata,
            metadata_destructor,
        ) != 0
        {
            if !metadata.is_null() {
                free_clif_function_data(metadata);
            }
            return Err(());
        }
    }
    Ok(())
}

unsafe fn register_function_owner_type_value(
    value: *mut ffi::PyObject,
    module_name: *mut ffi::PyObject,
    visited_types: &mut HashSet<usize>,
    shared_state: Option<&module_type::SharedModuleState>,
    module_runtime: Option<&ModuleRuntimeContext>,
) -> Result<(), ()> {
    if ffi::PyType_Check(value) != 0 {
        register_owner_types_from_type(
            value as *mut ffi::PyTypeObject,
            module_name,
            visited_types,
            shared_state,
            module_runtime,
        )?;
    }
    Ok(())
}

unsafe fn register_function_owner_type_indexed_key(
    globals: *mut ffi::PyObject,
    module_name: *mut ffi::PyObject,
    key: &str,
    visited_types: &mut HashSet<usize>,
    shared_state: Option<&module_type::SharedModuleState>,
    module_runtime: Option<&ModuleRuntimeContext>,
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
    let result = register_function_owner_type_value(
        value,
        module_name,
        visited_types,
        shared_state,
        module_runtime,
    );
    ffi::Py_DECREF(value);
    result
}

unsafe fn register_function_owner_types_for_globals(
    globals: *mut ffi::PyObject,
    module_name: *mut ffi::PyObject,
    indexed_module_keys: &[String],
    shared_state: Option<&module_type::SharedModuleState>,
    module_runtime: Option<&ModuleRuntimeContext>,
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
        register_function_owner_type_value(
            value,
            module_name,
            &mut visited_types,
            shared_state,
            module_runtime,
        )?;
    }
    for key in indexed_module_keys {
        register_function_owner_type_indexed_key(
            globals,
            module_name,
            key.as_str(),
            &mut visited_types,
            shared_state,
            module_runtime,
        )?;
    }
    Ok(())
}

unsafe fn owner_type_supports_early_registration(
    owner_type: *mut ffi::PyTypeObject,
    visited_types: &mut HashSet<usize>,
) -> bool {
    if !visited_types.insert(owner_type as usize) {
        return true;
    }
    if !owner_type_supports_direct_constructor_entry(owner_type) {
        return false;
    }

    let owner_dict = (*owner_type).tp_dict;
    if owner_dict.is_null() {
        return false;
    }
    let owner_module = ffi::PyDict_GetItemString(owner_dict, c"__module__".as_ptr());
    if owner_module.is_null() || ffi::PyUnicode_CheckExact(owner_module) == 0 {
        return false;
    }

    let mut position: ffi::Py_ssize_t = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while ffi::PyDict_Next(owner_dict, &mut position, &mut key, &mut value) != 0 {
        if ffi::PyType_Check(value) != 0
            && !owner_type_supports_early_registration(
                value.cast::<ffi::PyTypeObject>(),
                visited_types,
            )
        {
            return false;
        }
    }
    true
}

pub unsafe fn register_created_owner_type_from_namespace(
    owner_type: *mut ffi::PyObject,
    namespace_function: *mut ffi::PyObject,
) -> Result<(), ()> {
    if owner_type.is_null()
        || ffi::PyType_Check(owner_type) == 0
        || namespace_function.is_null()
        || ffi::PyFunction_Check(namespace_function) == 0
    {
        return Ok(());
    }

    let owner_type = owner_type.cast::<ffi::PyTypeObject>();
    if !owner_type_supports_early_registration(owner_type, &mut HashSet::new()) {
        return Ok(());
    }

    let metadata = PyFunction_GetSoacMetadata(namespace_function);
    if metadata.is_null() {
        return Ok(());
    }

    let (shared_state, compile_session, globals) = {
        let namespace_data = &*(metadata as *const PyFunctionJitExtra);
        let globals = namespace_data.function_env.globals_obj();
        if globals.is_null() {
            return set_runtime_error("class namespace function has no module globals");
        }
        ffi::Py_INCREF(globals);
        (
            Arc::clone(&namespace_data.module_state),
            Arc::clone(&namespace_data.compile_session),
            globals,
        )
    };
    let module_runtime = ModuleRuntimeContext {
        mod_ctx: jit::ModuleJitContext {
            shared_module_state: Arc::as_ptr(&shared_state),
            globals_obj: globals.cast::<c_void>(),
        },
        compile_session,
        shared_module_state_owner: Arc::clone(&shared_state),
    };

    let module_name = ffi::PyDict_GetItemString(globals, c"__name__".as_ptr());
    if module_name.is_null() {
        if ffi::PyErr_Occurred().is_null() {
            return set_runtime_error("class namespace module globals have no module name");
        }
        return Err(());
    }
    if ffi::PyUnicode_CheckExact(module_name) == 0 {
        return Ok(());
    }

    register_owner_types_from_type(
        owner_type,
        module_name,
        &mut HashSet::new(),
        Some(shared_state.as_ref()),
        Some(&module_runtime),
    )
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
    register_function_owner_types_for_globals(globals, module_name, &[], None, None)
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
    register_function_owner_types_for_globals(globals, module_name, indexed_module_keys, None, None)
}

pub unsafe fn register_function_owner_types_for_module_keys_with_constructor_entries(
    module: *mut ffi::PyObject,
    indexed_module_keys: &[String],
    shared_state: &module_type::SharedModuleState,
    module_runtime: &ModuleRuntimeContext,
) -> Result<(), ()> {
    if module.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"register_function_owner_types_for_module_keys_with_constructor_entries requires a module".as_ptr(),
        );
        return Err(());
    }
    let globals = ffi::PyModule_GetDict(module);
    let module_name = if globals.is_null() {
        ptr::null_mut()
    } else {
        ffi::PyDict_GetItemString(globals, c"__name__".as_ptr())
    };
    register_function_owner_types_for_globals(
        globals,
        module_name,
        indexed_module_keys,
        Some(shared_state),
        Some(module_runtime),
    )
}

unsafe fn ensure_clif_direct_entries_compiled(
    _py: Python<'_>,
    data: &mut PyFunctionJitExtra,
) -> Result<(), ()> {
    if data.function_env.compiled_function.is_none() {
        let ensure_start = Instant::now();
        let (compiled_function, function_qualname, function_block_count) = {
            let function = data.function()?;
            let function_block_count = function.blocks.len();
            let function_qualname = function.names.qualname.clone();
            let compiled_function_result = lookup_or_compile_direct_function_handle(
                &data.compile_session,
                &data.module_state,
                function,
                "vectorcall_function_body",
            );
            let compiled_function = match compiled_function_result {
                Ok(handle) => handle,
                Err(err) => {
                    if let Ok(c_msg) = CString::new(err) {
                        ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                    } else {
                        ffi::PyErr_SetString(
                            ffi::PyExc_RuntimeError,
                            b"failed to compile CLIF function body\0".as_ptr().cast(),
                        );
                    }
                    return Err(());
                }
            };
            (compiled_function, function_qualname, function_block_count)
        };
        attach_compiled_function_to_env(&mut data.function_env, compiled_function)?;
        let elapsed_ms = ensure_start.elapsed().as_secs_f64() * 1000.0;
        info!(
            "soac_jit_precompile module={} qualname={} blocks={} elapsed_ms={elapsed_ms:.3}",
            data.module_state.module_name, function_qualname, function_block_count,
        );
    }
    if data.function_env.direct_code_ptr().is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"compiled CLIF function is missing a direct entry pointer\0"
                .as_ptr()
                .cast(),
        );
        return Err(());
    }
    if data.function_env.default_direct_code_ptr().is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"compiled CLIF function is missing a default direct entry pointer\0"
                .as_ptr()
                .cast(),
        );
        return Err(());
    }
    if data.function_env.deopt_table_ptr().is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            b"compiled CLIF function is missing deopt metadata\0"
                .as_ptr()
                .cast(),
        );
        return Err(());
    }
    Ok(())
}

fn lookup_or_compile_direct_function_handle(
    compile_session: &Arc<CompileSession>,
    module_state: &Arc<module_type::SharedModuleState>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    log_event: &str,
) -> Result<Arc<jit::CompiledFunctionHandle>, String> {
    let compile_start = Instant::now();
    match module_state
        .lookup_or_compile_direct_function_handle(compile_session, function.function_id)
    {
        Ok(Some((handle, _compiled))) => Ok(handle),
        Ok(None) => {
            let block_ptrs = vec![ptr::null_mut::<c_void>(); function.blocks.len()];
            let module_constant_ptrs = module_state.module_constant_ptrs();
            let compile_result = unsafe {
                jit::compile_cranelift_run_bb_specialized_cached(
                    compile_session,
                    block_ptrs.as_slice(),
                    &module_state.lowered_module,
                    function,
                    &module_state.codegen_constants,
                    &module_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(module_state.as_ref()),
                )
            };
            match compile_result {
                Ok(result) => {
                    if result.compiled {
                        module_state.append_jit_codegen_log(
                            function,
                            log_event,
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
                        function,
                        log_event,
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
    }
}

pub unsafe fn resume_generator(
    resume_function: *mut ffi::PyObject,
    owner: *mut ffi::PyObject,
    preserved_state: *mut ffi::PyObject,
    send_value: *mut ffi::PyObject,
    resume_exc: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let Ok(data) = (unsafe { py_function_jit_extra(resume_function) }) else {
        return ptr::null_mut();
    };
    if data.function_template.function().body_params().len() != 4 {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"generator resume function expected a 4-argument resume body".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let py = unsafe { Python::assume_attached() };
    if unsafe { ensure_clif_direct_entries_compiled(py, data) }.is_err() {
        return ptr::null_mut();
    }
    let entry: unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
    ) -> *mut ffi::PyObject = unsafe { mem::transmute(data.function_env.direct_code_ptr()) };
    unsafe {
        entry(
            data.function_env.as_mut_ptr(),
            ffi::PyThreadState_Get().cast(),
            owner,
            preserved_state,
            send_value,
            resume_exc,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn soac_jit_resume_generator(
    resume_function: *mut ffi::PyObject,
    owner: *mut ffi::PyObject,
    preserved_state: *mut ffi::PyObject,
    send_value: *mut ffi::PyObject,
    resume_exc: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        if resume_function.is_null()
            || owner.is_null()
            || preserved_state.is_null()
            || send_value.is_null()
            || resume_exc.is_null()
        {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"JIT generator resume received a null argument".as_ptr(),
                );
            }
            return ptr::null_mut();
        }

        unsafe {
            resume_generator(
                resume_function,
                owner,
                preserved_state,
                send_value,
                resume_exc,
            )
        }
    })) {
        Ok(value) => value,
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"panic in soac_jit_resume_generator".as_ptr(),
                );
            }
            ptr::null_mut()
        }
    }
}

pub unsafe fn resume_async_generator(
    resume_function: *mut ffi::PyObject,
    owner: *mut ffi::PyObject,
    preserved_state: *mut ffi::PyObject,
    send_value: *mut ffi::PyObject,
    resume_exc: *mut ffi::PyObject,
    transport_sent: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let Ok(data) = (unsafe { py_function_jit_extra(resume_function) }) else {
        return ptr::null_mut();
    };
    if data.function_template.function().body_params().len() != 5 {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"async-generator resume function expected a 5-argument resume body".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let py = unsafe { Python::assume_attached() };
    if unsafe { ensure_clif_direct_entries_compiled(py, data) }.is_err() {
        return ptr::null_mut();
    }
    let entry: unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
    ) -> *mut ffi::PyObject = unsafe { mem::transmute(data.function_env.direct_code_ptr()) };
    unsafe {
        entry(
            data.function_env.as_mut_ptr(),
            ffi::PyThreadState_Get().cast(),
            owner,
            preserved_state,
            send_value,
            resume_exc,
            transport_sent,
        )
    }
}

fn attach_compiled_function_to_env(
    function_env: &mut FunctionEnv,
    compiled_function: Arc<jit::CompiledFunctionHandle>,
) -> Result<(), ()> {
    let direct_code_ptr = compiled_function
        .direct_code_ptr()
        .map(|ptr| ptr as *const u8)
        .map_err(|err| set_runtime_error_message(&err))?;
    let default_direct_code_ptr = compiled_function
        .default_direct_code_ptr()
        .map(|ptr| ptr as *const u8)
        .map_err(|err| set_runtime_error_message(&err))?;
    let deopt_table_ptr = compiled_function
        .direct_deopt_table_ptr()
        .map(|ptr| ptr as *const c_void)
        .map_err(|err| set_runtime_error_message(&err))?;
    function_env.set_direct_code_ptr(direct_code_ptr);
    function_env.set_default_direct_code_ptr(default_direct_code_ptr);
    function_env.set_deopt_table_ptr(deopt_table_ptr);
    function_env.compiled_function = Some(compiled_function);
    Ok(())
}

fn prepared_vectorcall_trampoline(
    function_template: &FunctionInstantiationTemplate,
    compile_session: &Arc<CompileSession>,
    param_count: usize,
) -> Result<jit::VectorcallEntryFn, String> {
    if let Some(prepared) = function_template
        .prepared_vectorcall_trampoline
        .get()
        .filter(|prepared| prepared.matches(compile_session.id(), param_count))
    {
        return Ok(prepared.entry);
    }

    let entry = compile_session
        .process_jit()?
        .vectorcall_trampoline(compile_session, param_count)?;
    if function_template
        .prepared_vectorcall_trampoline
        .get()
        .is_none()
    {
        let _ =
            function_template
                .prepared_vectorcall_trampoline
                .set(PreparedVectorcallTrampoline {
                    compile_session_id: compile_session.id(),
                    param_count,
                    entry,
                });
    }
    Ok(entry)
}

pub(crate) unsafe fn attach_ready_clif_direct_entry(
    function: *mut ffi::PyObject,
) -> Result<bool, ()> {
    let data = unsafe { py_function_jit_extra(function)? };
    if data.function_env.compiled_function.is_some() {
        return Ok(true);
    }
    let registered_code_is_current =
        unsafe { (*function.cast::<ffi::PyFunctionObject>()).func_code == data.registered_code };
    let key = PreparedDirectEntryKey {
        compile_session_id: data.compile_session.id(),
        code_ptr: data.registered_code as usize,
        code_version: unsafe { jit::raw_py_code_version(data.registered_code) },
    };
    let ready = {
        let function = data.function()?;
        if entry_interpreter_vectorcall_requested(function) {
            return Ok(false);
        }
        if let Some(prepared) = data
            .function_template
            .prepared_direct_entry
            .get()
            .filter(|prepared| registered_code_is_current && prepared.key == key)
        {
            Some(Arc::clone(&prepared.handle))
        } else {
            let engine = data
                .compile_session
                .process_jit()
                .map_err(|err| set_runtime_error_message(&err))?;
            engine
                .lookup_ready_direct_function(function)
                .map_err(|err| set_runtime_error_message(&err))?
        }
    };
    let Some(compiled_function) = ready else {
        return Ok(false);
    };
    if registered_code_is_current && data.function_template.prepared_direct_entry.get().is_none() {
        let _ = data
            .function_template
            .prepared_direct_entry
            .set(PreparedDirectEntry {
                key,
                handle: Arc::clone(&compiled_function),
            });
    }
    attach_compiled_function_to_env(&mut data.function_env, compiled_function)?;
    Ok(true)
}

unsafe fn ensure_clif_vectorcall_compiled(
    py: Python<'_>,
    callable: *mut ffi::PyObject,
    data: &mut PyFunctionJitExtra,
) -> Result<(), ()> {
    unsafe { ensure_clif_direct_entries_compiled(py, data)? };
    if *data.function()?.lowered_kind() != FunctionKind::Function {
        return Ok(());
    }
    if data.compiled_vectorcall_entry.is_none() {
        let param_count = data.function_template.binding_plan().param_count();
        let entry = match prepared_vectorcall_trampoline(
            data.function_template.as_ref(),
            &data.compile_session,
            param_count,
        ) {
            Ok(value) => value,
            Err(err) => {
                if let Ok(c_msg) = CString::new(err) {
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
                } else {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        b"failed to compile direct CLIF vectorcall trampoline\0"
                            .as_ptr()
                            .cast(),
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
    let plan = data.function_template.binding_plan();
    if out_len != plan.param_count() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"bound CLIF argument count did not match direct entry arity".as_ptr(),
        );
        return Err(());
    }

    if plan.binds_exact_positional(nargsf, kwnames) {
        if out_len != 0 && out_args.is_null() {
            return initialize_output_args(out_args, out_len);
        }
        if out_len != 0 && args.is_null() {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"missing vectorcall argument array in CLIF function binding".as_ptr(),
            );
            return Err(());
        }
        for position in 0..out_len {
            let value = *args.add(position);
            if value.is_null() {
                cleanup_output_args(out_args, position);
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"null vectorcall positional argument\0".as_ptr().cast(),
                );
                return Err(());
            }
            write_output_arg_from_borrowed(out_args, position, value);
        }
        return Ok(());
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
                b"null vectorcall positional argument\0".as_ptr().cast(),
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
                    b"null vectorcall positional vararg\0".as_ptr().cast(),
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
                b"null vectorcall keyword argument\0".as_ptr().cast(),
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
                b"invalid direct vectorcall bind input\0".as_ptr().cast(),
            );
            return 0;
        }
        let data = &mut *(data_ptr as *mut PyFunctionJitExtra);
        if data
            .refresh_runtime_objects_from_current_function(callable as *mut ffi::PyObject)
            .is_err()
        {
            return 0;
        }
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
                    b"panic in bind_direct_args_from_vectorcall\0"
                        .as_ptr()
                        .cast(),
                );
            }
            0
        }
    }
}

#[cold]
pub(crate) unsafe extern "C" fn vectorcall_previous_for_changed_code(
    callable: *mut c_void,
    args: *const *mut c_void,
    nargsf: usize,
    kwnames: *mut c_void,
    data_ptr: *mut c_void,
) -> *mut c_void {
    match panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if callable.is_null()
            || data_ptr.is_null()
            || ffi::PyFunction_Check(callable.cast::<ffi::PyObject>()) == 0
        {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"invalid changed-code vectorcall fallback".as_ptr(),
            );
            return ptr::null_mut();
        }

        let function = callable.cast::<ffi::PyFunctionObject>();
        let data = &mut *data_ptr.cast::<PyFunctionJitExtra>();
        let Some(previous_vectorcall) = data.previous_vectorcall else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"changed function is missing its original vectorcall".as_ptr(),
            );
            return ptr::null_mut();
        };
        PyFunction_SetVectorcall(function, Some(previous_vectorcall));
        jit::invalidate_py_function_soac_function_id(function);
        previous_vectorcall(
            callable.cast::<ffi::PyObject>(),
            args.cast::<*mut ffi::PyObject>(),
            nargsf,
            kwnames.cast::<ffi::PyObject>(),
        )
        .cast::<c_void>()
    })) {
        Ok(result) => result,
        Err(payload) => {
            let message = format!(
                "panic in vectorcall_previous_for_changed_code: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr()) };
            } else {
                unsafe {
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"panic in changed-code vectorcall fallback".as_ptr(),
                    )
                };
            }
            ptr::null_mut()
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
                b"invalid vectorcall function env compile input\0"
                    .as_ptr()
                    .cast(),
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
                    b"panic in vectorcall_compile_function_env\0"
                        .as_ptr()
                        .cast(),
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
        let _ = callable;
        if data_ptr.is_null() {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"invalid direct function env compile input\0"
                    .as_ptr()
                    .cast(),
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
                    b"panic in direct_compile_function_env\0".as_ptr().cast(),
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
    let data_ptr = make_clif_function_data(function, function_id, module_runtime, None)?;
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
    unsafe { register_clif_vectorcall_with_template(function, function_id, module_runtime, None) }
}

unsafe fn register_clif_vectorcall_with_template(
    function: *mut ffi::PyObject,
    function_id: RuntimeFunctionId,
    module_runtime: jit::ModuleRuntimeContext,
    known_function_template: Option<Arc<FunctionInstantiationTemplate>>,
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
        if *data.function()?.lowered_kind() != FunctionKind::Function {
            data.compiled_vectorcall_entry = None;
            PyFunction_SetVectorcall(func, Some(generator_factory_vectorcall));
            return Ok(());
        }
        if entry_interpreter_vectorcall_requested(data.function()?) {
            data.compiled_vectorcall_entry = None;
            PyFunction_SetVectorcall(func, Some(entry_interpreter_vectorcall));
            return Ok(());
        }
        let param_count = data.function_template.binding_plan().param_count();
        let entry = prepared_vectorcall_trampoline(
            data.function_template.as_ref(),
            &data.compile_session,
            param_count,
        )
        .map_err(|err| {
            if let Ok(c_msg) = CString::new(err) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    b"failed to compile shared CLIF vectorcall trampoline\0"
                        .as_ptr()
                        .cast(),
                );
            }
        })?;
        data.compiled_vectorcall_entry = Some(entry);
        let vectorcall_entry: ffi::vectorcallfunc = std::mem::transmute(entry);
        PyFunction_SetVectorcall(func, Some(vectorcall_entry));
        return Ok(());
    }
    let function_template = match known_function_template {
        Some(template) if template.function().function_id == function_id => template,
        Some(_) => {
            return set_runtime_error(
                "known function template does not match its vectorcall function identifier",
            );
        }
        None => module_runtime
            .shared_module_state_owner
            .lookup_function_template(function_id)
            .map_err(|err| set_runtime_error_message(&err))?
            .ok_or_else(|| {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"no specialized JIT plan found while registering vectorcall".as_ptr(),
                );
            })?,
    };
    let blockpy_function = function_template.function();
    let blockpy_function_kind = *blockpy_function.lowered_kind();
    let blockpy_function_param_count = blockpy_function.params.len();
    if blockpy_function_kind != FunctionKind::Function {
        let data_ptr = make_clif_function_data(
            function,
            function_id,
            module_runtime,
            Some(function_template),
        )?;
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
        PyFunction_SetVectorcall(func, Some(generator_factory_vectorcall));
        return Ok(());
    }
    if entry_interpreter_vectorcall_requested(blockpy_function) {
        let data_ptr = make_clif_function_data(
            function,
            function_id,
            module_runtime,
            Some(function_template),
        )?;
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
        let data_ptr = make_clif_function_data(
            function,
            function_id,
            module_runtime,
            Some(function_template),
        )?;
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
    let entry = prepared_vectorcall_trampoline(
        function_template.as_ref(),
        &module_runtime.compile_session,
        blockpy_function_param_count,
    )
    .map_err(|err| {
        if let Ok(c_msg) = CString::new(err) {
            ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
        } else {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                b"failed to compile shared CLIF vectorcall trampoline\0"
                    .as_ptr()
                    .cast(),
            );
        }
    })?;

    let data_ptr = make_clif_function_data(
        function,
        function_id,
        module_runtime,
        Some(function_template),
    )?;
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
    function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
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
    let data = match unsafe { py_function_jit_extra(callable) } {
        Ok(data) => data,
        Err(()) => return ptr::null_mut(),
    };
    if unsafe { (*callable.cast::<ffi::PyFunctionObject>()).func_code } != data.registered_code {
        return unsafe {
            vectorcall_previous_for_changed_code(
                callable.cast::<c_void>(),
                args.cast::<*mut c_void>(),
                nargsf,
                kwnames.cast::<c_void>(),
                ptr::from_mut(data).cast::<c_void>(),
            )
            .cast::<ffi::PyObject>()
        };
    }
    if unsafe { data.refresh_runtime_objects_from_current_function(callable) }.is_err() {
        return ptr::null_mut();
    }
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

fn generator_kind_tag(kind: FunctionKind) -> Option<i64> {
    match kind {
        FunctionKind::Generator => Some(0),
        FunctionKind::Coroutine => Some(1),
        FunctionKind::AsyncGenerator => Some(2),
        FunctionKind::Function => None,
    }
}

unsafe fn tuple_set_owned(
    tuple: *mut ffi::PyObject,
    index: usize,
    value: *mut ffi::PyObject,
) -> Result<(), ()> {
    if value.is_null() {
        return Err(());
    }
    if ffi::PyTuple_SetItem(tuple, index as ffi::Py_ssize_t, value) != 0 {
        ffi::Py_DECREF(value);
        return Err(());
    }
    Ok(())
}

unsafe fn interned_generator_factory_key(
    py: Python<'_>,
    name: &'static std::ffi::CStr,
) -> Result<Py<PyAny>, ()> {
    let value = unsafe { ffi::PyUnicode_InternFromString(name.as_ptr()) };
    if value.is_null() {
        return Err(());
    }
    Ok(unsafe { Bound::<PyAny>::from_owned_ptr(py, value) }.unbind())
}

unsafe fn exact_c_function_has_name(
    function: *mut ffi::PyObject,
    expected_name: &'static std::ffi::CStr,
) -> bool {
    if function.is_null() || unsafe { ffi::PyCFunction_CheckExact(function) } == 0 {
        return false;
    }
    let name = unsafe { ffi::PyObject_GetAttrString(function, c"__name__".as_ptr()) };
    if name.is_null() {
        unsafe { ffi::PyErr_Clear() };
        return false;
    }
    let matches = unsafe { ffi::PyUnicode_CheckExact(name) } != 0
        && unsafe { ffi::PyUnicode_CompareWithASCIIString(name, expected_name.as_ptr()) } == 0;
    unsafe { ffi::Py_DECREF(name) };
    matches
}

unsafe fn prepare_generator_factory(
    py: Python<'_>,
    data: &PyFunctionJitExtra,
    helper: *mut ffi::PyObject,
) -> Result<Option<PreparedGeneratorFactory>, ()> {
    let Some(helper_function_id) = (unsafe { registered_clif_function_id(helper)? }) else {
        return Ok(None);
    };
    let helper_data = unsafe { py_function_jit_extra(helper)? };
    if helper_data.compile_session.id() != data.compile_session.id()
        || helper_data.function_id != helper_function_id
        || helper_data.module_state.module_name != "soac.runtime"
        || helper_data.function_template.function().names.qualname != "make_generator_instance"
    {
        return Ok(None);
    }
    let Some(original_helper_code) = helper_data
        .module_state
        .lookup_original_code(helper_function_id)
    else {
        return Ok(None);
    };
    let helper_function = helper.cast::<ffi::PyFunctionObject>();
    let helper_code = unsafe { (*helper_function).func_code };
    if helper_code != original_helper_code.as_ptr() || helper_code != helper_data.registered_code {
        return Ok(None);
    }
    let runtime_globals = unsafe { (*helper_function).func_globals };
    if runtime_globals != helper_data.function_env.globals_obj() {
        return Ok(None);
    }

    let getattr_key = unsafe { interned_generator_factory_key(py, c"getattr")? };
    let preserved_state_key =
        unsafe { interned_generator_factory_key(py, c"make_preserved_state")? };
    let generator_class_key = unsafe { interned_generator_factory_key(py, c"ClosureGenerator")? };
    let code_template_key = unsafe { interned_generator_factory_key(py, c"code_template_gen")? };
    let extension_key = unsafe { interned_generator_factory_key(py, c"_soac_ext")? };
    let init_key = unsafe { interned_generator_factory_key(py, c"__init__")? };

    let builtin_getattr = unsafe { ffi::PyDict_GetItem(runtime_globals, getattr_key.as_ptr()) };
    let builtins = helper_data.function_env.builtins_obj();
    if builtins.is_null()
        || unsafe { ffi::PyDict_Check(builtins) } == 0
        || builtin_getattr != unsafe { ffi::PyDict_GetItem(builtins, getattr_key.as_ptr()) }
        || !unsafe { exact_c_function_has_name(builtin_getattr, c"getattr") }
    {
        return Ok(None);
    }

    let preserved_state_factory =
        unsafe { ffi::PyDict_GetItem(runtime_globals, preserved_state_key.as_ptr()) };
    let extension = unsafe { ffi::PyDict_GetItem(runtime_globals, extension_key.as_ptr()) };
    if extension.is_null() || unsafe { ffi::PyModule_CheckExact(extension) } == 0 {
        return Ok(None);
    }
    let extension_dict = unsafe { ffi::PyModule_GetDict(extension) };
    if extension_dict.is_null()
        || preserved_state_factory
            != unsafe { ffi::PyDict_GetItem(extension_dict, preserved_state_key.as_ptr()) }
        || !unsafe { exact_c_function_has_name(preserved_state_factory, c"make_preserved_state") }
    {
        return Ok(None);
    }
    let preserved_state_owner = unsafe { ffi::PyCFunction_GetSelf(preserved_state_factory) };
    if !preserved_state_owner.is_null() && preserved_state_owner != extension {
        return Ok(None);
    }

    let generator_class =
        unsafe { ffi::PyDict_GetItem(runtime_globals, generator_class_key.as_ptr()) };
    if generator_class.is_null() || unsafe { ffi::PyType_CheckExact(generator_class) } == 0 {
        return Ok(None);
    }
    let owner_type = generator_class.cast::<ffi::PyTypeObject>();
    let owner_dict = unsafe { (*owner_type).tp_dict };
    if owner_dict.is_null() {
        return Ok(None);
    }
    let init = unsafe { ffi::PyDict_GetItem(owner_dict, init_key.as_ptr()) };
    if init.is_null() {
        return Ok(None);
    }
    let Some(init_function_id) = (unsafe { registered_clif_function_id(init)? }) else {
        return Ok(None);
    };
    let Some(original_init_code) = helper_data
        .module_state
        .lookup_original_code(init_function_id)
    else {
        return Ok(None);
    };
    if unsafe { (*init.cast::<ffi::PyFunctionObject>()).func_code } != original_init_code.as_ptr() {
        return Ok(None);
    }
    let Some(constructor_function_id) = soac_ir_blockpy::constructor_entry_function_id_for_init(
        &helper_data.module_state.lowered_module,
        init_function_id,
    ) else {
        return Ok(None);
    };
    if unsafe { registered_clif_type_function_id(generator_class)? }
        != Some(constructor_function_id)
    {
        return Ok(None);
    }
    if unsafe { (*owner_type).tp_version_tag } == 0
        && unsafe { PyUnstable_Type_AssignVersionTag(owner_type) } == 0
    {
        if !unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Err(());
        }
        return Ok(None);
    }
    let generator_class_version = unsafe { (*owner_type).tp_version_tag };
    if generator_class_version == 0 {
        return Ok(None);
    }

    let code_template = unsafe { ffi::PyDict_GetItem(runtime_globals, code_template_key.as_ptr()) };
    if code_template.is_null() {
        return Ok(None);
    }

    Ok(Some(PreparedGeneratorFactory {
        compile_session_id: data.compile_session.id(),
        helper_function_id,
        helper: helper as usize,
        helper_code: helper_code as usize,
        runtime_globals: runtime_globals as usize,
        builtin_getattr: builtin_getattr as usize,
        preserved_state_factory: preserved_state_factory as usize,
        generator_class: generator_class as usize,
        generator_class_version,
        code_template: code_template as usize,
        getattr_key,
        preserved_state_key,
        generator_class_key,
        code_template_key,
    }))
}

unsafe fn generator_factory_still_canonical(
    prepared: &PreparedGeneratorFactory,
    data: &PyFunctionJitExtra,
    helper: *mut ffi::PyObject,
) -> bool {
    if data.compile_session.id() != prepared.compile_session_id
        || helper as usize != prepared.helper
        || unsafe { ffi::PyFunction_Check(helper) } == 0
        || unsafe { (*helper.cast::<ffi::PyFunctionObject>()).func_code } as usize
            != prepared.helper_code
        || unsafe { PyFunction_GetSoacFunctionId(helper) }
            != prepared.helper_function_id.to_packed_runtime_u64()
        || unsafe {
            jit::raw_py_function_activation_is_observed(prepared.helper_code as *mut ffi::PyObject)
        }
    {
        return false;
    }

    let runtime_globals = prepared.runtime_globals as *mut ffi::PyObject;
    if unsafe { ffi::PyDict_GetItem(runtime_globals, prepared.getattr_key.as_ptr()) } as usize
        != prepared.builtin_getattr
        || unsafe { ffi::PyDict_GetItem(runtime_globals, prepared.preserved_state_key.as_ptr()) }
            as usize
            != prepared.preserved_state_factory
        || unsafe { ffi::PyDict_GetItem(runtime_globals, prepared.generator_class_key.as_ptr()) }
            as usize
            != prepared.generator_class
        || unsafe { ffi::PyDict_GetItem(runtime_globals, prepared.code_template_key.as_ptr()) }
            as usize
            != prepared.code_template
    {
        return false;
    }

    let generator_class = prepared.generator_class as *mut ffi::PyTypeObject;
    (unsafe { (*generator_class).tp_version_tag }) == prepared.generator_class_version
}

unsafe fn try_make_source_generator_instance_direct(
    py: Python<'_>,
    function_obj: *mut ffi::PyObject,
    data: &PyFunctionJitExtra,
    helper: *mut ffi::PyObject,
    bound_args: &mut [*mut ffi::PyObject],
    yieldfrom_slot: usize,
    throw_context_slot: usize,
    closed_slot: usize,
) -> Result<Option<*mut ffi::PyObject>, ()> {
    let function = data.function_template.function();
    if *function.lowered_kind() != FunctionKind::Generator
        || function.names.display_name != "<genexpr>"
    {
        return Ok(None);
    }
    let raw_function = function_obj.cast::<ffi::PyFunctionObject>();
    let source_code = unsafe { (*raw_function).func_code };
    if data
        .module_state
        .lookup_original_code(function.function_id)
        .map(Py::as_ptr)
        != Some(source_code)
        || unsafe { jit::raw_py_code_flags(source_code) } & ffi::CO_GENERATOR == 0
    {
        return Ok(None);
    }

    let function_name = unsafe { (*raw_function).func_name };
    let function_qualname = unsafe { (*raw_function).func_qualname };
    if function_name.is_null()
        || function_qualname.is_null()
        || unsafe { ffi::PyUnicode_CheckExact(function_name) } == 0
        || unsafe { ffi::PyUnicode_CheckExact(function_qualname) } == 0
        || !unsafe {
            jit::raw_py_code_has_function_names(source_code, function_name, function_qualname)
        }
        || unsafe { ffi::PyUnicode_CompareWithASCIIString(function_name, c"<genexpr>".as_ptr()) }
            != 0
    {
        return Ok(None);
    }

    let Some(layout) = function.public_storage_layout() else {
        return Ok(None);
    };
    if layout.preserved_slots.iter().any(|slot| {
        !matches!(
            (slot.storage, &slot.init),
            (
                PreservedSlotStorage::I64,
                ClosureInit::RuntimePcUnstarted
                    | ClosureInit::RuntimeAbruptKindFallthrough
                    | ClosureInit::RuntimeZero
            ) | (
                PreservedSlotStorage::PyObjectOrNull,
                ClosureInit::Parameter | ClosureInit::RuntimeNone | ClosureInit::Deferred
            ) | (
                PreservedSlotStorage::PyCellObject,
                ClosureInit::Parameter | ClosureInit::EmptyCell
            )
        )
    }) {
        return Ok(None);
    }

    let template = data.function_template.as_ref();
    let prepared = match template.prepared_generator_factory.get() {
        Some(prepared) => prepared,
        None => {
            if unsafe { ffi::PyFunction_Check(helper) } == 0 {
                return Ok(None);
            }
            let helper_code = unsafe { (*helper.cast::<ffi::PyFunctionObject>()).func_code };
            if unsafe { jit::raw_py_function_activation_is_observed(helper_code) } {
                return Ok(None);
            }
            let Some(prepared) = (unsafe { prepare_generator_factory(py, data, helper)? }) else {
                return Ok(None);
            };
            // Preparation can allocate Python objects and must not hold the OnceLock during
            // callbacks or reentrant generator creation.
            let _ = template.prepared_generator_factory.set(prepared);
            template
                .prepared_generator_factory
                .get()
                .expect("a prepared generator factory should be initialized")
        }
    };
    if !unsafe { generator_factory_still_canonical(prepared, data, helper) } {
        return Ok(None);
    }

    let mut state =
        preserved_state::PreservedStateBuilder::with_capacity(layout.preserved_slots.len())?;
    for slot in &layout.preserved_slots {
        match (&slot.storage, &slot.init) {
            (PreservedSlotStorage::I64, ClosureInit::RuntimePcUnstarted) => state.push_i64(1),
            (
                PreservedSlotStorage::I64,
                ClosureInit::RuntimeAbruptKindFallthrough | ClosureInit::RuntimeZero,
            ) => state.push_i64(0),
            (
                PreservedSlotStorage::PyObjectOrNull,
                ClosureInit::RuntimeNone | ClosureInit::Deferred,
            ) => {
                let value = unsafe { ffi::Py_None() };
                unsafe { ffi::Py_INCREF(value) };
                unsafe { state.push_owned_object(value) };
            }
            (PreservedSlotStorage::PyCellObject, ClosureInit::EmptyCell) => {
                let cell = unsafe { PyCell_New(ptr::null_mut()) };
                if cell.is_null() {
                    return Err(());
                }
                unsafe { state.push_owned_object(cell) };
            }
            (
                storage @ (PreservedSlotStorage::PyObjectOrNull
                | PreservedSlotStorage::PyCellObject),
                ClosureInit::Parameter,
            ) => {
                let Some(param_index) = data
                    .function_template
                    .binding_plan()
                    .param_index(slot.logical_name.as_str())
                else {
                    return set_runtime_error("preserved parameter slot has no public parameter");
                };
                let mut value = bound_args[param_index];
                if value.is_null() {
                    let Some(default_slot) = data
                        .function_template
                        .binding_plan()
                        .params
                        .get(param_index)
                        .and_then(|param| param.default_slot)
                    else {
                        return set_runtime_error("preserved parameter slot was not bound");
                    };
                    value = data.function_env.runtime_object(default_slot);
                    if value.is_null() {
                        return set_runtime_error("preserved parameter slot default was not bound");
                    }
                }
                if *storage == PreservedSlotStorage::PyCellObject {
                    value = unsafe { PyCell_New(value) };
                    if value.is_null() {
                        return Err(());
                    }
                } else {
                    unsafe { ffi::Py_INCREF(value) };
                }
                unsafe { state.push_owned_object(value) };
            }
            _ => unreachable!("unsupported preserved slots are rejected before direct creation"),
        }
    }

    let preserved_values = unsafe { state.into_capsule() };
    if preserved_values.is_null() {
        return Err(());
    }
    unsafe { cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len()) };

    let mut slot_indices = [ptr::null_mut(); 3];
    for (index, slot) in [yieldfrom_slot, throw_context_slot, closed_slot]
        .into_iter()
        .enumerate()
    {
        let value = unsafe { ffi::PyLong_FromSize_t(slot) };
        if value.is_null() {
            for previous in slot_indices {
                unsafe { ffi::Py_XDECREF(previous) };
            }
            unsafe { ffi::Py_DECREF(preserved_values) };
            return Err(());
        }
        slot_indices[index] = value;
    }

    let constructor_args = [
        function_obj,
        function_name,
        function_qualname,
        source_code,
        preserved_values,
        slot_indices[0],
        slot_indices[1],
        slot_indices[2],
    ];
    let result = unsafe {
        ffi::PyObject_Vectorcall(
            prepared.generator_class as *mut ffi::PyObject,
            constructor_args.as_ptr(),
            constructor_args.len(),
            ptr::null_mut(),
        )
    };
    for slot in slot_indices {
        unsafe { ffi::Py_DECREF(slot) };
    }
    unsafe { ffi::Py_DECREF(preserved_values) };
    if result.is_null() {
        return Err(());
    }
    tracing::debug!(
        target: "soac_generator_direct_state",
        path = "direct",
        temporary_python_tuples = 0,
        function_id = ?function.function_id,
        qualname = function.names.qualname.as_str(),
        "generator_factory_direct_preserved_state",
    );
    Ok(Some(result))
}

unsafe fn make_generator_instance_from_vectorcall(
    function_obj: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> Result<*mut ffi::PyObject, ()> {
    let py = Python::assume_attached();
    let data = py_function_jit_extra(function_obj)?;
    let function = data.function()?;
    let Some(kind_tag) = generator_kind_tag(*function.lowered_kind()) else {
        return set_runtime_error(
            "generator factory vectorcall expected a generator-like function",
        );
    };
    let Some(layout) = function.public_storage_layout() else {
        return set_runtime_error("generator-like function is missing preserved-state layout");
    };
    tracing::info!(
        target: "soac_generator_preserved_layout",
        function_id = ?function.function_id,
        qualname = function.names.qualname.as_str(),
        preserved_slots = ?layout
            .preserved_slots
            .iter()
            .map(|slot| (
                slot.logical_name.as_str(),
                slot.storage_name.as_str(),
                slot.storage,
                slot.init.clone(),
            ))
            .collect::<Vec<_>>(),
        "generator_factory_public_preserved_layout",
    );
    let Some(yieldfrom_slot) = layout
        .preserved_slots
        .iter()
        .position(|slot| slot.logical_name == "_dp_yieldfrom")
    else {
        return set_runtime_error(
            "generator-like function is missing _dp_yieldfrom preserved slot",
        );
    };
    let Some(throw_context_slot) = layout
        .preserved_slots
        .iter()
        .position(|slot| slot.logical_name == "_dp_throw_context")
    else {
        return set_runtime_error(
            "generator-like function is missing _dp_throw_context preserved slot",
        );
    };
    let Some(closed_slot) = layout
        .preserved_slots
        .iter()
        .position(|slot| slot.logical_name == "_dp_is_closed")
    else {
        return set_runtime_error(
            "generator-like function is missing _dp_is_closed preserved slot",
        );
    };

    let mut bound_args = vec![ptr::null_mut(); data.function_template.binding_plan().param_count()];
    bind_function_args_to_output(
        data,
        args,
        nargsf,
        kwnames,
        bound_args.as_mut_ptr(),
        bound_args.len(),
    )?;

    let make_instance_ptr = data
        .module_state
        .runtime_name_owned_cached(RuntimeName::MakeGeneratorInstance);
    if make_instance_ptr.is_null() {
        cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
        return Err(());
    }
    let make_instance: Bound<'_, PyAny> = Bound::from_owned_ptr(py, make_instance_ptr);
    match try_make_source_generator_instance_direct(
        py,
        function_obj,
        data,
        make_instance.as_ptr(),
        &mut bound_args,
        yieldfrom_slot,
        throw_context_slot,
        closed_slot,
    ) {
        Ok(Some(generator)) => {
            cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
            return Ok(generator);
        }
        Ok(None) => {}
        Err(()) => {
            cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
            return Err(());
        }
    }

    let initial_values = ffi::PyTuple_New(layout.preserved_slots.len() as ffi::Py_ssize_t);
    if initial_values.is_null() {
        cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
        return Err(());
    }
    let slot_kinds = ffi::PyTuple_New(layout.preserved_slots.len() as ffi::Py_ssize_t);
    if slot_kinds.is_null() {
        ffi::Py_DECREF(initial_values);
        cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
        return Err(());
    }
    for (slot_index, slot) in layout.preserved_slots.iter().enumerate() {
        let value = match slot.init {
            ClosureInit::Parameter => {
                let Some(param_index) = data
                    .function_template
                    .binding_plan()
                    .param_index(slot.logical_name.as_str())
                else {
                    ffi::Py_DECREF(initial_values);
                    ffi::Py_DECREF(slot_kinds);
                    cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
                    return set_runtime_error("preserved parameter slot has no public parameter");
                };
                let value = bound_args[param_index];
                let value = if value.is_null() {
                    let Some(default_slot) = data
                        .function_template
                        .binding_plan()
                        .params
                        .get(param_index)
                        .and_then(|param| param.default_slot)
                    else {
                        ffi::Py_DECREF(initial_values);
                        ffi::Py_DECREF(slot_kinds);
                        cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
                        return set_runtime_error("preserved parameter slot was not bound");
                    };
                    let default_value = data.function_env.runtime_object(default_slot);
                    if default_value.is_null() {
                        ffi::Py_DECREF(initial_values);
                        ffi::Py_DECREF(slot_kinds);
                        cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
                        return set_runtime_error("preserved parameter slot default was not bound");
                    }
                    default_value
                } else {
                    value
                };
                match slot.storage {
                    PreservedSlotStorage::PyCellObject => {
                        let cell = PyCell_New(value);
                        if cell.is_null() {
                            ffi::Py_DECREF(initial_values);
                            ffi::Py_DECREF(slot_kinds);
                            cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
                            return Err(());
                        }
                        cell
                    }
                    PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64 => {
                        ffi::Py_INCREF(value);
                        value
                    }
                }
            }
            ClosureInit::RuntimePcUnstarted => ffi::PyLong_FromLongLong(1),
            ClosureInit::RuntimeAbruptKindFallthrough => ffi::PyLong_FromLongLong(0),
            ClosureInit::RuntimeZero => ffi::PyLong_FromLongLong(0),
            ClosureInit::RuntimeNone | ClosureInit::Deferred => {
                let none = ffi::Py_None();
                ffi::Py_INCREF(none);
                none
            }
            ClosureInit::EmptyCell if slot.storage == PreservedSlotStorage::PyCellObject => {
                let cell = PyCell_New(ptr::null_mut());
                if cell.is_null() {
                    ffi::Py_DECREF(initial_values);
                    ffi::Py_DECREF(slot_kinds);
                    cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
                    return Err(());
                }
                cell
            }
            ClosureInit::InheritedCapture | ClosureInit::EmptyCell => {
                ffi::Py_DECREF(initial_values);
                ffi::Py_DECREF(slot_kinds);
                cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());
                return set_runtime_error("closure-only init reached preserved-state layout");
            }
        };
        tuple_set_owned(initial_values, slot_index, value)?;
        let kind = match slot.storage {
            PreservedSlotStorage::PyObjectOrNull => 0,
            PreservedSlotStorage::PyCellObject => 0,
            PreservedSlotStorage::I64 => 1,
        };
        tuple_set_owned(slot_kinds, slot_index, ffi::PyLong_FromLongLong(kind))?;
    }
    cleanup_output_args(bound_args.as_mut_ptr(), bound_args.len());

    let initial_values = Bound::from_owned_ptr(py, initial_values);
    let slot_kinds = Bound::from_owned_ptr(py, slot_kinds);
    let raw_function = function_obj.cast::<ffi::PyFunctionObject>();
    let (function_name, function_qualname) = if jit::raw_py_code_has_function_names(
        (*raw_function).func_code,
        (*raw_function).func_name,
        (*raw_function).func_qualname,
    ) {
        (
            Bound::<PyAny>::from_borrowed_ptr(py, (*raw_function).func_name),
            Bound::<PyAny>::from_borrowed_ptr(py, (*raw_function).func_qualname),
        )
    } else {
        (
            pyo3::types::PyString::new(py, function.names.display_name.as_str()).into_any(),
            pyo3::types::PyString::new(py, function.names.qualname.as_str()).into_any(),
        )
    };
    let function_obj = Bound::from_borrowed_ptr(py, function_obj);
    let result = make_instance
        .call1((
            function_obj,
            kind_tag,
            function_name,
            function_qualname,
            initial_values,
            slot_kinds,
            yieldfrom_slot,
            throw_context_slot,
            closed_slot,
        ))
        .map_err(|err| {
            err.restore(py);
        })?;
    Ok(result.into_ptr())
}

unsafe extern "C" fn generator_factory_vectorcall(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let data = match unsafe { py_function_jit_extra(callable) } {
        Ok(data) => data,
        Err(()) => return ptr::null_mut(),
    };
    if unsafe { (*callable.cast::<ffi::PyFunctionObject>()).func_code } != data.registered_code {
        return unsafe {
            vectorcall_previous_for_changed_code(
                callable.cast::<c_void>(),
                args.cast::<*mut c_void>(),
                nargsf,
                kwnames.cast::<c_void>(),
                ptr::from_mut(data).cast::<c_void>(),
            )
            .cast::<ffi::PyObject>()
        };
    }
    if unsafe { data.refresh_runtime_objects_from_current_function(callable) }.is_err() {
        return ptr::null_mut();
    }
    if ffi::Py_EnterRecursiveCall(c" while calling a Python object".as_ptr()) != 0 {
        return ptr::null_mut();
    }
    let _recursive_call = EntryInterpreterRecursiveCallGuard;
    match panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        make_generator_instance_from_vectorcall(callable, args, nargsf, kwnames)
    })) {
        Ok(Ok(result)) => result,
        Ok(Err(())) => ptr::null_mut(),
        Err(payload) => {
            let message = format!(
                "panic in generator_factory_vectorcall: {}",
                panic_payload_to_string(payload)
            );
            if let Ok(c_msg) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c_msg.as_ptr());
            } else {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"panic in generator_factory_vectorcall".as_ptr(),
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
        data.function_env.builtins_obj().cast::<c_void>(),
        data.function_env.runtime_objects_ptr().cast::<c_void>(),
        data.function_template.entry_plan(),
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

    #[test]
    fn repeated_closure_registration_reuses_its_known_template_and_vectorcall_trampoline() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();

        Python::attach(|py| {
            let source = "def outer(offset):\n    def inner(value):\n        return offset + value\n    return inner\n";
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
                .expect("closure registration fixture should lower")
                .blockpy_module;
            let module_state = module_type::build_shared_state_for_testing(
                py,
                lowered,
                "template_aware_registration_test",
                "",
            )
            .expect("closure registration fixture should build shared module state");
            let function = module_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == "outer.<locals>.inner")
                .expect("closure registration fixture should contain its nested function");
            let function_id = function.function_id;
            let template = module_state
                .lookup_function_template(function_id)
                .expect("closure registration template lookup should succeed")
                .expect("closure registration template should exist");

            let globals = PyDict::new(py);
            let builtins = py
                .import("builtins")
                .expect("closure registration fixture should import builtins");
            globals
                .set_item("__builtins__", &builtins)
                .expect("closure registration globals should accept builtins");
            globals
                .set_item("__name__", "template_aware_registration_test")
                .expect("closure registration globals should accept their module name");
            builtins
                .getattr("exec")
                .and_then(|exec| exec.call1((source, &globals, &globals)))
                .expect("closure registration fixture should define actual Python functions");
            let outer = globals
                .get_item("outer")
                .expect("closure registration outer lookup should succeed")
                .expect("closure registration outer function should exist");
            let first = outer
                .call1((3,))
                .expect("the first real closure should be created");
            let second = outer
                .call1((9,))
                .expect("the second real closure should be created");
            let public_closure = outer
                .call1((15,))
                .expect("the public-registration closure should be created");
            assert_ne!(
                first.as_ptr(),
                second.as_ptr(),
                "separate registrations must preserve distinct actual Python function objects"
            );

            let session = Arc::new(CompileSession::new());
            session
                .retain_shared_module_state(Arc::clone(&module_state))
                .expect("the closure registration session should retain its real module state");
            let runtime = || {
                unsafe { ffi::Py_INCREF(globals.as_ptr()) };
                jit::ModuleRuntimeContext {
                    mod_ctx: jit::ModuleJitContext {
                        shared_module_state: Arc::as_ptr(&module_state),
                        globals_obj: globals.as_ptr().cast(),
                    },
                    compile_session: Arc::clone(&session),
                    shared_module_state_owner: Arc::clone(&module_state),
                }
            };

            for closure in [&first, &second] {
                unsafe {
                    register_clif_vectorcall_with_template(
                        closure.as_ptr(),
                        function_id,
                        runtime(),
                        Some(Arc::clone(&template)),
                    )
                }
                .expect("each actual closure should register through the production path");
                let metadata = unsafe { py_function_jit_extra(closure.as_ptr()) }
                    .expect("each actual closure should expose its registered JIT metadata");
                assert!(
                    Arc::ptr_eq(&metadata.function_template, &template),
                    "registration must retain the already-known immutable function template"
                );
            }

            unsafe { register_clif_vectorcall(public_closure.as_ptr(), function_id, runtime()) }
                .expect("the existing public registration entrypoint should remain supported");
            let public_metadata = unsafe { py_function_jit_extra(public_closure.as_ptr()) }
                .expect("public registration should expose its normal JIT metadata");
            assert!(Arc::ptr_eq(&public_metadata.function_template, &template));

            let prepared = template
                .prepared_vectorcall_trampoline
                .get()
                .expect("repeated actual closure registration should cache its shared trampoline");
            assert!(prepared.matches(session.id(), 1));
            assert!(!prepared.matches(session.id(), 2));
            let other_session = Arc::new(CompileSession::new());
            assert!(!prepared.matches(other_session.id(), 1));
            let other_session_entry =
                prepared_vectorcall_trampoline(template.as_ref(), &other_session, 1)
                    .expect("a different compile session should prepare its own live trampoline");
            assert_ne!(
                other_session_entry as usize, prepared.entry as usize,
                "a cached trampoline must never be reused across distinct compile sessions"
            );
            let other_arity_entry = prepared_vectorcall_trampoline(template.as_ref(), &session, 2)
                .expect("a different argument shape should prepare its own live trampoline");
            assert_ne!(
                other_arity_entry as usize, prepared.entry as usize,
                "a cached trampoline must never be reused for another argument shape"
            );
            assert!(
                template
                    .prepared_vectorcall_trampoline
                    .get()
                    .expect("the original cached trampoline should remain available")
                    .matches(session.id(), 1),
                "session and arity mismatches must not overwrite the original positive cache"
            );

            let first_metadata = unsafe { py_function_jit_extra(first.as_ptr()) }
                .expect("the first closure metadata should remain live");
            let second_metadata = unsafe { py_function_jit_extra(second.as_ptr()) }
                .expect("the second closure metadata should remain live");
            assert_ne!(
                first_metadata.function_env_ptr, second_metadata.function_env_ptr,
                "sharing a template or trampoline must never share per-function environments"
            );
            let closure_slot = template.runtime_data_layout().closure_cell_slot(0);
            let first_cell = first_metadata.function_env.runtime_object(closure_slot);
            let second_cell = second_metadata.function_env.runtime_object(closure_slot);
            assert!(!first_cell.is_null());
            assert!(!second_cell.is_null());
            assert_ne!(
                first_cell, second_cell,
                "separately instantiated closures must retain their own captured cells"
            );
            let first_cell = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, first_cell) };
            let second_cell = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, second_cell) };
            assert_eq!(
                first_cell
                    .getattr("cell_contents")
                    .and_then(|value| value.extract::<i64>())
                    .expect("the first closure cell should expose its original value"),
                3
            );
            assert_eq!(
                second_cell
                    .getattr("cell_contents")
                    .and_then(|value| value.extract::<i64>())
                    .expect("the second closure cell should expose its original value"),
                9
            );
            assert_eq!(
                first_metadata
                    .compiled_vectorcall_entry
                    .expect("the first closure should have a vectorcall trampoline")
                    as usize,
                prepared.entry as usize
            );
            assert_eq!(
                second_metadata
                    .compiled_vectorcall_entry
                    .expect("the second closure should have a vectorcall trampoline")
                    as usize,
                prepared.entry as usize
            );
            assert_eq!(
                public_metadata
                    .compiled_vectorcall_entry
                    .expect("the public registration should have a vectorcall trampoline")
                    as usize,
                prepared.entry as usize
            );
        });
    }

    #[test]
    fn exact_positional_binding_selects_only_fully_supplied_ordered_parameters() {
        let module = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def zero():
    return 1

def ordinary(first, second):
    return first + second

def positional_only(first, /, second):
    return first + second

def defaulted(first, second=2):
    return first + second

def keyword_only(first, *, second):
    return first + second

def variadic(first, *remaining):
    return first

def keyword_variadic(first, **remaining):
    return first

def make_closure(captured):
    def closure(value):
        return captured + value
    return closure

def generated(value):
    yield value
"#,
        )
        .expect("exact-positional binding fixture should lower")
        .blockpy_module;
        let plan_for = |qualname: &str| {
            let function = module
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == qualname)
                .unwrap_or_else(|| panic!("lowered binding fixture should contain {qualname}"));
            DirectArgBindingPlan::from_function(function)
        };
        let no_keywords = ptr::null_mut();
        let present_keywords = NonNull::<ffi::PyObject>::dangling().as_ptr();

        assert!(plan_for("zero").binds_exact_positional(0, no_keywords));
        assert!(plan_for("ordinary").binds_exact_positional(2, no_keywords));
        assert!(
            plan_for("ordinary")
                .binds_exact_positional(2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET, no_keywords),
            "the vectorcall offset flag is not part of the positional argument count"
        );
        assert!(plan_for("positional_only").binds_exact_positional(2, no_keywords));
        assert!(plan_for("defaulted").binds_exact_positional(2, no_keywords));
        assert!(plan_for("make_closure.<locals>.closure").binds_exact_positional(1, no_keywords));
        assert!(plan_for("generated").binds_exact_positional(1, no_keywords));

        assert!(!plan_for("ordinary").binds_exact_positional(1, no_keywords));
        assert!(!plan_for("ordinary").binds_exact_positional(3, no_keywords));
        assert!(!plan_for("ordinary").binds_exact_positional(2, present_keywords));
        assert!(!plan_for("defaulted").binds_exact_positional(1, no_keywords));
        assert!(!plan_for("keyword_only").binds_exact_positional(2, no_keywords));
        assert!(!plan_for("variadic").binds_exact_positional(2, no_keywords));
        assert!(!plan_for("keyword_variadic").binds_exact_positional(2, no_keywords));
    }

    #[test]
    fn exact_positional_binding_preserves_owned_references_and_cleans_only_written_prefix() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| {
            let module = soac_lowering::lower_python_to_blockpy_for_testing(
                "def zero():\n    return 1\n\ndef three(first, second, third):\n    return first\n",
            )
            .expect("actual vectorcall binding fixture should lower")
            .blockpy_module;
            let module_state = module_type::build_shared_state_for_testing(
                py,
                module,
                "exact_positional_binding_test",
                "",
            )
            .expect("actual vectorcall binding fixture should build module state");
            let globals = PyDict::new(py);
            let builtins = PyDict::new(py);
            let data_for = |qualname: &str| {
                let function = module_state
                    .lowered_module
                    .callable_defs
                    .iter()
                    .find(|function| function.names.qualname == qualname)
                    .unwrap_or_else(|| panic!("binding fixture should contain {qualname}"));
                let function_template = module_state
                    .lookup_function_template(function.function_id)
                    .expect("binding fixture template lookup should succeed")
                    .expect("binding fixture should have a function template");
                let mut function_env = Box::new(
                    unsafe {
                        FunctionEnv::new(
                            globals.as_ptr(),
                            builtins.as_ptr(),
                            module_state.late_bound_owner_fields.cells.as_ptr(),
                            Vec::new().into_boxed_slice(),
                        )
                    }
                    .expect("binding fixture function environment should allocate"),
                );
                PyFunctionJitExtra {
                    function_env_ptr: function_env.as_mut_ptr(),
                    function_id: function.function_id,
                    function_env,
                    function_template,
                    compile_session: Arc::new(CompileSession::new()),
                    module_state: Arc::clone(&module_state),
                    compiled_vectorcall_entry: None,
                    previous_vectorcall: None,
                    registered_code: ptr::null_mut(),
                    registered_defaults: ptr::null_mut(),
                    registered_kwdefaults: ptr::null_mut(),
                }
            };

            let zero = data_for("zero");
            assert!(
                unsafe {
                    bind_function_args_to_output(
                        &zero,
                        ptr::null(),
                        ffi::PY_VECTORCALL_ARGUMENTS_OFFSET,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        0,
                    )
                }
                .is_ok(),
                "zero-argument calls must accept null buffers and the vectorcall offset flag"
            );

            let three = data_for("three");
            let first = PyList::empty(py);
            let second = PyList::empty(py);
            let third = PyList::empty(py);
            let first_refcount = unsafe { ffi::Py_REFCNT(first.as_ptr()) };
            let second_refcount = unsafe { ffi::Py_REFCNT(second.as_ptr()) };
            let third_refcount = unsafe { ffi::Py_REFCNT(third.as_ptr()) };
            let args = [first.as_ptr(), second.as_ptr(), third.as_ptr()];
            let sentinel = NonNull::<ffi::PyObject>::dangling().as_ptr();
            let mut output = [sentinel; 3];
            assert!(
                unsafe {
                    bind_function_args_to_output(
                        &three,
                        args.as_ptr(),
                        3 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET,
                        ptr::null_mut(),
                        output.as_mut_ptr(),
                        output.len(),
                    )
                }
                .is_ok(),
                "fully positional arguments should bind in declaration order"
            );
            assert_eq!(output, args);
            assert_eq!(
                unsafe { ffi::Py_REFCNT(first.as_ptr()) },
                first_refcount + 1
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(second.as_ptr()) },
                second_refcount + 1
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(third.as_ptr()) },
                third_refcount + 1
            );
            unsafe { cleanup_output_args(output.as_mut_ptr(), output.len()) };
            assert_eq!(unsafe { ffi::Py_REFCNT(first.as_ptr()) }, first_refcount);
            assert_eq!(unsafe { ffi::Py_REFCNT(second.as_ptr()) }, second_refcount);
            assert_eq!(unsafe { ffi::Py_REFCNT(third.as_ptr()) }, third_refcount);

            let malformed = [first.as_ptr(), ptr::null_mut(), third.as_ptr()];
            let mut partial_output = [sentinel; 3];
            assert!(
                unsafe {
                    bind_function_args_to_output(
                        &three,
                        malformed.as_ptr(),
                        malformed.len(),
                        ptr::null_mut(),
                        partial_output.as_mut_ptr(),
                        partial_output.len(),
                    )
                }
                .is_err(),
                "a malformed positional argument must fail without reading unwritten slots"
            );
            let malformed_error = pyo3::PyErr::fetch(py);
            assert!(
                malformed_error
                    .to_string()
                    .contains("null vectorcall positional argument")
            );
            assert!(partial_output[0].is_null());
            assert_eq!(partial_output[1], sentinel);
            assert_eq!(partial_output[2], sentinel);
            assert_eq!(unsafe { ffi::Py_REFCNT(first.as_ptr()) }, first_refcount);
            assert_eq!(unsafe { ffi::Py_REFCNT(third.as_ptr()) }, third_refcount);

            assert!(
                unsafe {
                    bind_function_args_to_output(
                        &three,
                        ptr::null(),
                        3,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        3,
                    )
                }
                .is_err()
            );
            let missing_output_error = pyo3::PyErr::fetch(py);
            assert!(
                missing_output_error
                    .to_string()
                    .contains("missing output buffer for direct CLIF function arguments"),
                "output-buffer validation must retain precedence over a missing argument array"
            );

            assert!(
                unsafe {
                    bind_function_args_to_output(
                        &three,
                        ptr::null(),
                        3,
                        ptr::null_mut(),
                        output.as_mut_ptr(),
                        output.len(),
                    )
                }
                .is_err()
            );
            let missing_args_error = pyo3::PyErr::fetch(py);
            assert!(
                missing_args_error
                    .to_string()
                    .contains("missing vectorcall argument array in CLIF function binding")
            );
        });
    }

    #[test]
    fn prepared_direct_entry_key_rejects_other_sessions_codes_and_versions() {
        let first_session = CompileSession::new();
        let second_session = CompileSession::new();
        let original = PreparedDirectEntryKey {
            compile_session_id: first_session.id(),
            code_ptr: 7,
            code_version: 11,
        };
        assert_eq!(
            original,
            PreparedDirectEntryKey {
                compile_session_id: first_session.id(),
                code_ptr: 7,
                code_version: 11,
            },
            "a prepared entry must be reusable for the same immutable source code"
        );
        assert_ne!(
            original,
            PreparedDirectEntryKey {
                compile_session_id: second_session.id(),
                ..original
            },
            "a prepared entry must never cross compile sessions"
        );
        assert_ne!(
            original,
            PreparedDirectEntryKey {
                code_ptr: 13,
                ..original
            },
            "a prepared entry must not attach to a different source code object"
        );
        assert_ne!(
            original,
            PreparedDirectEntryKey {
                code_version: 17,
                ..original
            },
            "a prepared entry must not attach to another source code version"
        );
    }

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
    fn function_owner_registry_initialization_does_not_install_global_watcher() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "function_owner_registry_initialization_does_not_install_global_watcher",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|_| {
            let registry =
                function_owner_type_registry().expect("owner type registry should initialize");
            assert_eq!(
                registry.watcher_id.load(Ordering::Acquire),
                -1,
                "creating the owner registry must not install a process-wide function watcher"
            );
        });
    }

    #[test]
    fn trusted_runtime_owner_tracking_does_not_install_global_watcher() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "trusted_runtime_owner_tracking_does_not_install_global_watcher",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (_module, cls) = make_test_module(py);
            let owner_type = cls.as_ptr().cast::<ffi::PyTypeObject>();
            let function = class_dict_function(owner_type, c"f");
            register_owner_type_for_function(function, owner_type, false)
                .expect("trusted runtime owner should remain registered");
            let registry =
                function_owner_type_registry().expect("owner type registry should initialize");
            assert_eq!(
                registry.watcher_id.load(Ordering::Acquire),
                -1,
                "trusted compiler-owned runtime classes must not activate the global watcher"
            );
            assert!(
                registry
                    .registered_owner_types_by_function
                    .lock()
                    .expect("owner type registry lock should succeed")
                    .contains_key(&(function as usize)),
                "runtime owner metadata must remain available for method specialization"
            );
            ffi::Py_DECREF(function);
        });
    }

    #[test]
    fn mutable_source_owner_tracking_installs_global_watcher_lazily() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "mutable_source_owner_tracking_installs_global_watcher_lazily",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module(py);
            register_function_owner_types_for_module(module.as_ptr())
                .expect("mutable source owner should register");
            let registry =
                function_owner_type_registry().expect("owner type registry should initialize");
            assert!(
                registry.watcher_id.load(Ordering::Acquire) >= 0,
                "the first mutable source class must activate function mutation watching"
            );
            let owner_type = cls.as_ptr().cast::<ffi::PyTypeObject>();
            let function = class_dict_function(owner_type, c"f");
            assert!(
                registry
                    .registered_owner_types_by_function
                    .lock()
                    .expect("owner type registry lock should succeed")
                    .contains_key(&(function as usize)),
                "mutable source methods must retain type-version invalidation"
            );
            ffi::Py_DECREF(function);
        });
    }

    #[test]
    fn function_owner_watcher_skips_unregistered_functions_without_weakrefs() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let registry =
                function_owner_type_registry().expect("owner type registry should initialize");
            let function = make_test_function(py);
            assert!(PyFunction_GetSoacMetadata(function).is_null());
            assert!(
                !function_may_have_registered_owner_types(function.cast::<ffi::PyFunctionObject>(),),
                "a transient source function without a weakref cannot be registered"
            );
            assert!(
                !registry
                    .registered_owner_types_by_function
                    .lock()
                    .expect("owner type registry lock should succeed")
                    .contains_key(&(function as usize))
            );
            assert_eq!(
                function_owner_type_watcher_callback(
                    PY_FUNCTION_EVENT_DESTROY,
                    function.cast::<ffi::PyFunctionObject>(),
                    ptr::null_mut(),
                ),
                0,
                "an unregistered function must skip owner-type cleanup"
            );
            ffi::Py_DECREF(function);
        });
    }

    #[test]
    fn function_owner_watcher_retains_uncompiled_registered_source_methods() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module(py);
            register_function_owner_types_for_module(module.as_ptr())
                .expect("owner type registration should succeed");

            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let function = class_dict_function(owner_type, c"f");
            assert!(
                PyFunction_GetSoacMetadata(function).is_null(),
                "ordinary registered source methods need not have JIT metadata"
            );
            assert!(
                function_may_have_registered_owner_types(function.cast::<ffi::PyFunctionObject>(),),
                "the owner registry's retained weakref must keep cleanup eligible"
            );
            let registry =
                function_owner_type_registry().expect("owner type registry should initialize");
            assert!(
                registry
                    .registered_owner_types_by_function
                    .lock()
                    .expect("owner type registry lock should succeed")
                    .contains_key(&(function as usize)),
                "registered uncompiled source methods must remain tracked"
            );
            ffi::Py_DECREF(function);
        });
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
    fn invalidating_function_identity_preserves_live_soac_metadata() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            TEST_SOAC_METADATA_DROPS.store(0, Ordering::SeqCst);
            let function = make_test_function(py);
            let function_id = RuntimeFunctionId::from_raw_parts(7, 12);
            let metadata = Box::into_raw(Box::new(456usize)) as *mut c_void;

            assert_eq!(
                PyFunction_SetSoacMetadata(
                    function,
                    function_id.to_packed_runtime_u64(),
                    metadata,
                    Some(free_test_soac_metadata),
                ),
                0,
                "setting function metadata should succeed"
            );

            jit::invalidate_py_function_soac_function_id(function.cast::<ffi::PyFunctionObject>());

            assert_eq!(
                PyFunction_GetSoacMetadata(function),
                metadata,
                "invalidating the direct-call identity must retain live function metadata"
            );
            assert_eq!(
                PyFunction_GetSoacFunctionId(function),
                0,
                "invalidated functions must fail compiled direct-call guards"
            );
            assert_eq!(
                registered_clif_function_id(function)
                    .expect("invalidated function identity lookup should succeed"),
                None,
                "invalidated functions must not remain registered direct-call targets"
            );
            assert_eq!(
                TEST_SOAC_METADATA_DROPS.load(Ordering::SeqCst),
                0,
                "active JIT metadata must not be destroyed by identity invalidation"
            );

            assert_eq!(
                PyFunction_SetSoacMetadata(function, 0, ptr::null_mut(), None),
                0,
                "clearing retained metadata should succeed"
            );
            assert_eq!(
                TEST_SOAC_METADATA_DROPS.load(Ordering::SeqCst),
                1,
                "retained metadata must still be released exactly once"
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
    fn owner_type_registration_skips_constructor_type_function_id_for_custom_new() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        initialize_test_python();
        Python::attach(|py| unsafe {
            let (module, cls) = make_test_module_with_source(
                py,
                "class C:\n    def __new__(cls, value):\n        return super().__new__(cls)\n    def __init__(self, value):\n        self.value = value\n",
            );
            let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
            let init_function = class_dict_function(owner_type, c"__init__");
            let function_id = RuntimeFunctionId::from_raw_parts(10, 5);
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
                0,
                "custom __new__ should keep constructor type metadata unset"
            );
            assert_eq!(
                registered_clif_type_function_id(cls.as_ptr())
                    .expect("type function id lookup should succeed"),
                None,
                "custom __new__ should not decode a constructor entry id"
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
    fn exact_constructor_owner_lookup_includes_custom_new_types() {
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
            assert_eq!(owners.len(), 1, "expected one constructor owner type");
            assert_eq!(owners[0].owner_type, owner_type);
            assert_eq!(owners[0].init_function_obj, init_function);
            assert_ne!(owners[0].type_version, 0);
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
