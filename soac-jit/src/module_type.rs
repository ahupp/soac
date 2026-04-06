use crate::counter::{Counter, CounterEntry};
use crate::counter_dump::{CounterDumpRecord, CounterDumpRow};
use crate::module_constants::ModuleCodegenConstants;
use crate::module_globals::ModuleGlobalCache;
use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyAnyMethods;
use soac_blockpy::block_py::{
    BlockPyFunction, BlockPyModule, CounterDef, CounterId, CounterScope, CounterSite, FunctionId,
};
use soac_blockpy::passes::CodegenBlockPyPass;
use std::collections::HashMap;
use std::env;
use std::ffi::{c_int, c_void};
use std::fs::OpenOptions;
use std::io::Write;
use std::mem::MaybeUninit;
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::sync::Mutex;

pub struct SoacExtModuleDataRef<'a> {
    pub shared_state: &'a SharedModuleState,
}

pub struct SharedModuleState {
    pub lowered_module: BlockPyModule<CodegenBlockPyPass>,
    pub module_name: String,
    pub package_name: String,
    pub codegen_constants: ModuleCodegenConstants,
    function_index_by_id: HashMap<FunctionId, usize>,
    module_constant_objs: Vec<Py<PyAny>>,
    counter_slots_by_id: Box<[CounterRuntimeSlot]>,
    counter_values: Box<[u64]>,
    call_target_counter_values: Box<[Mutex<Counter<2, u64>>]>,
    compiled_direct_runner_handles: Mutex<HashMap<FunctionId, DirectRunnerCacheEntry>>,
}

#[derive(Clone, Copy)]
enum DirectRunnerCacheEntry {
    InProgress,
    Ready(*mut c_void),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CounterStorageKey {
    This(CounterId),
    Shared {
        scope: CounterScope,
        site: CounterSite,
        kind: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CounterRuntimeSlot {
    Scalar(usize),
    CallHotTargets(usize),
}

impl SharedModuleState {
    pub fn lookup_function(
        &self,
        function_id: FunctionId,
    ) -> Option<&BlockPyFunction<CodegenBlockPyPass>> {
        let function_index = self.function_index_by_id.get(&function_id).copied()?;
        let function = self.lowered_module.callable_defs.get(function_index)?;
        assert_eq!(function.function_id, function_id);
        Some(function)
    }

    pub(crate) fn module_constant_ptrs(&self) -> Vec<*mut ffi::PyObject> {
        self.module_constant_objs
            .iter()
            .map(|obj| obj.as_ptr())
            .collect()
    }

    pub(crate) fn counter_ptrs(&self) -> Vec<*mut u64> {
        self.counter_slots_by_id
            .iter()
            .map(|slot| match slot {
                CounterRuntimeSlot::Scalar(slot) => {
                    &self.counter_values[*slot] as *const u64 as *mut u64
                }
                CounterRuntimeSlot::CallHotTargets(_) => ptr::null_mut(),
            })
            .collect()
    }

    pub fn counter_values(&self) -> &[u64] {
        &self.counter_values
    }

    pub fn counter_value(&self, counter_id: CounterId) -> u64 {
        let Some(slot) = self.counter_slots_by_id.get(counter_id.0).copied() else {
            return 0;
        };
        match slot {
            CounterRuntimeSlot::Scalar(slot) => {
                self.counter_values.get(slot).copied().unwrap_or_default()
            }
            CounterRuntimeSlot::CallHotTargets(_) => 0,
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn record_call_target_counter(
        &self,
        counter_id: CounterId,
        value: u64,
    ) -> Result<(), String> {
        let Some(slot) = self.counter_slots_by_id.get(counter_id.0).copied() else {
            return Err(format!(
                "missing counter slot for counter id {}",
                counter_id.0
            ));
        };
        let CounterRuntimeSlot::CallHotTargets(slot) = slot else {
            return Err(format!(
                "counter id {} is not a call target counter",
                counter_id.0
            ));
        };
        let counter = self
            .call_target_counter_values
            .get(slot)
            .ok_or_else(|| format!("missing call target counter slot {}", slot))?;
        let mut counter = counter
            .lock()
            .map_err(|_| "call target counter lock poisoned".to_string())?;
        counter.record(value);
        Ok(())
    }

    fn call_target_counter_snapshot(
        &self,
        counter_id: CounterId,
    ) -> Option<Vec<CounterEntry<u64>>> {
        let CounterRuntimeSlot::CallHotTargets(slot) =
            self.counter_slots_by_id.get(counter_id.0).copied()?
        else {
            return None;
        };
        let counter = self.call_target_counter_values.get(slot)?;
        let counter = counter.lock().ok()?;
        Some(counter.snapshot())
    }

    pub(crate) fn lookup_or_compile_direct_code_ptr(
        &self,
        function_id: FunctionId,
    ) -> Result<Option<*mut c_void>, String> {
        {
            let mut cache = self
                .compiled_direct_runner_handles
                .lock()
                .map_err(|_| "compiled direct runner cache lock poisoned".to_string())?;
            match cache.get(&function_id).copied() {
                Some(DirectRunnerCacheEntry::Ready(handle)) => {
                    return crate::jit::compiled_direct_code_ptr(handle).map(Some);
                }
                Some(DirectRunnerCacheEntry::InProgress) => return Ok(None),
                None => {
                    cache.insert(function_id, DirectRunnerCacheEntry::InProgress);
                }
            }
        }
        let function = self
            .lookup_function(function_id)
            .cloned()
            .ok_or_else(|| format!("missing direct-call target for function_id={function_id}"))?;
        let blocks = vec![ptr::null_mut::<c_void>(); function.blocks.len()];
        let module_constant_ptrs = self.module_constant_ptrs();
        let counter_ptrs = self.counter_ptrs();
        let handle = unsafe {
            crate::jit::compile_cranelift_run_bb_specialized_cached(
                blocks.as_slice(),
                &self.lowered_module,
                &function,
                &self.codegen_constants,
                &self.lowered_module.counter_defs,
                &module_constant_ptrs,
                &counter_ptrs,
                Some(self),
            )
            .map_err(|err| {
                format!(
                    "{err} [direct_target={} id={}]",
                    function.names.qualname, function.function_id
                )
            })?
        };
        let code_ptr = match crate::jit::compiled_direct_code_ptr(handle) {
            Ok(code_ptr) => code_ptr,
            Err(err) => {
                let mut cache = self
                    .compiled_direct_runner_handles
                    .lock()
                    .map_err(|_| "compiled direct runner cache lock poisoned".to_string())?;
                cache.remove(&function_id);
                return Err(err);
            }
        };
        let mut cache = self
            .compiled_direct_runner_handles
            .lock()
            .map_err(|_| "compiled direct runner cache lock poisoned".to_string())?;
        cache.insert(function_id, DirectRunnerCacheEntry::Ready(handle));
        Ok(Some(code_ptr))
    }

    pub fn counter_dump_record(&self) -> Option<CounterDumpRecord> {
        if self.lowered_module.counter_defs.is_empty() {
            return None;
        }

        let mut rows = Vec::new();
        for counter in &self.lowered_module.counter_defs {
            let (
                site_kind,
                function_id,
                current_function_id,
                instr_id,
                function_qualname,
                block_label,
            ) = match &counter.site {
                CounterSite::BlockEntry {
                    function_id,
                    block_label,
                } => {
                    let function = self.lookup_function(*function_id);
                    (
                        "block_entry".to_string(),
                        Some(*function_id),
                        Some(*function_id),
                        None,
                        function
                            .map(|function| function.names.qualname.clone())
                            .or_else(|| Some("<missing-function>".to_string())),
                        Some(block_label.to_string()),
                    )
                }
                CounterSite::Runtime {
                    function_id,
                    instr_id,
                } => (
                    "runtime".to_string(),
                    Some(function_id.unwrap_or(FunctionId::global())),
                    Some(function_id.unwrap_or(FunctionId::global())),
                    *instr_id,
                    function_id.and_then(|function_id| {
                        self.lookup_function(function_id)
                            .map(|function| function.names.qualname.clone())
                    }),
                    None,
                ),
            };

            let base_row = CounterDumpRow {
                counter_id: u32::try_from(counter.id.0).expect("counter ids should fit in u32"),
                scope: counter_scope_name(counter.scope).to_string(),
                kind: counter.kind.clone(),
                site_kind,
                function_id,
                current_function_id,
                instr_id,
                function_qualname,
                block_label,
                value: 0,
                observed_value: None,
                max_overcount: None,
            };

            if counter_uses_call_target_storage(counter) {
                let snapshot = self
                    .call_target_counter_snapshot(counter.id)
                    .unwrap_or_default();
                if snapshot.is_empty() {
                    rows.push(base_row);
                } else {
                    for entry in snapshot {
                        let mut row = base_row.clone();
                        row.value = entry.approx_count;
                        row.observed_value = Some(entry.value);
                        row.max_overcount = Some(entry.max_overcount);
                        rows.push(row);
                    }
                }
            } else {
                let mut row = base_row;
                row.value = self.counter_value(counter.id);
                rows.push(row);
            }
        }

        Some(CounterDumpRecord {
            module_name: self.module_name.clone(),
            package_name: (!self.package_name.is_empty()).then(|| self.package_name.clone()),
            rows,
        })
    }

    pub fn append_counter_dump_file(&self, path: &Path) -> Result<(), String> {
        let Some(record) = self.counter_dump_record() else {
            return Ok(());
        };
        let bytes = record.encode()?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|err| format!("failed to open {}: {err}", path.display()))?;
        file.write_all(bytes.as_slice())
            .map_err(|err| format!("failed to write {}: {err}", path.display()))
    }
}

impl Drop for SharedModuleState {
    fn drop(&mut self) {
        let cache = self
            .compiled_direct_runner_handles
            .get_mut()
            .expect("compiled direct runner cache lock should not be poisoned during drop");
        for handle in cache.drain().map(|(_, handle)| handle) {
            if let DirectRunnerCacheEntry::Ready(handle) = handle {
                unsafe { crate::jit::free_cranelift_run_bb_specialized_cached(handle) };
            }
        }
    }
}

fn counter_scope_name(scope: CounterScope) -> &'static str {
    match scope {
        CounterScope::This => "this",
        CounterScope::Function => "function",
        CounterScope::Global => "global",
    }
}

fn counter_storage_key(counter: &CounterDef) -> PyResult<CounterStorageKey> {
    match counter.scope {
        CounterScope::This => Ok(CounterStorageKey::This(counter.id)),
        CounterScope::Function | CounterScope::Global => Ok(CounterStorageKey::Shared {
            scope: counter.scope,
            site: counter.site.clone(),
            kind: counter.kind.clone(),
        }),
    }
}

fn counter_uses_call_target_storage(counter: &CounterDef) -> bool {
    matches!(
        counter.kind.as_str(),
        "call_hot_targets" | "operator_hot_shapes"
    )
}

fn build_counter_storage(
    counter_defs: &[CounterDef],
) -> PyResult<(
    Box<[CounterRuntimeSlot]>,
    Box<[u64]>,
    Box<[Mutex<Counter<2, u64>>]>,
)> {
    let mut slots_by_id = vec![None; counter_defs.len()];
    let mut scalar_slot_by_key = HashMap::new();
    let mut call_target_slot_by_key = HashMap::new();
    let mut counter_values = Vec::new();
    let mut call_target_counter_values = Vec::new();
    for counter in counter_defs {
        if counter.id.0 >= slots_by_id.len() {
            return Err(PyRuntimeError::new_err(format!(
                "counter id {} is out of range for {} counter defs",
                counter.id.0,
                counter_defs.len()
            )));
        }
        let key = counter_storage_key(counter)?;
        let slot = if counter_uses_call_target_storage(counter) {
            let slot = if let Some(slot) = call_target_slot_by_key.get(&key).copied() {
                slot
            } else {
                let slot = call_target_counter_values.len();
                call_target_counter_values.push(Mutex::new(Counter::new()));
                call_target_slot_by_key.insert(key, slot);
                slot
            };
            CounterRuntimeSlot::CallHotTargets(slot)
        } else {
            let slot = if let Some(slot) = scalar_slot_by_key.get(&key).copied() {
                slot
            } else {
                let slot = counter_values.len();
                counter_values.push(0);
                scalar_slot_by_key.insert(key, slot);
                slot
            };
            CounterRuntimeSlot::Scalar(slot)
        };
        slots_by_id[counter.id.0] = Some(slot);
    }
    Ok((
        slots_by_id
            .into_iter()
            .map(|slot| slot.expect("every counter id should map to a runtime counter slot"))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        counter_values.into_boxed_slice(),
        call_target_counter_values.into_boxed_slice(),
    ))
}

#[cfg(test)]
pub(crate) fn build_shared_state_for_testing(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenBlockPyPass>,
    module_name: &str,
    package_name: &str,
) -> PyResult<Arc<SharedModuleState>> {
    let function_index_by_id = build_function_index_by_id(&lowered_module)?;
    let (counter_slots_by_id, counter_values, call_target_counter_values) =
        build_counter_storage(&lowered_module.counter_defs)?;
    let codegen_constants = ModuleCodegenConstants::collect_from_module(&lowered_module);
    let module_constant_objs = codegen_constants.build_python_constants(py)?;
    Ok(Arc::new(SharedModuleState {
        lowered_module,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        codegen_constants,
        function_index_by_id,
        module_constant_objs,
        counter_slots_by_id,
        counter_values,
        call_target_counter_values,
        compiled_direct_runner_handles: Mutex::new(HashMap::new()),
    }))
}

fn build_function_index_by_id(
    module: &BlockPyModule<CodegenBlockPyPass>,
) -> PyResult<HashMap<FunctionId, usize>> {
    let mut function_index_by_id = HashMap::with_capacity(module.callable_defs.len());
    for (function_index, function) in module.callable_defs.iter().enumerate() {
        if function_index_by_id
            .insert(function.function_id, function_index)
            .is_some()
        {
            return Err(PyRuntimeError::new_err(format!(
                "duplicate function id {} in shared module state ({})",
                function.function_id, function.names.qualname
            )));
        }
    }
    Ok(function_index_by_id)
}

#[repr(C)]
struct SoacExtModuleState {
    initialized: bool,
    shared_state: MaybeUninit<Arc<SharedModuleState>>,
    global_cache: MaybeUninit<Arc<ModuleGlobalCache>>,
    global_cache_initialized: bool,
}

impl SoacExtModuleState {
    unsafe fn init(
        &mut self,
        py: Python<'_>,
        lowered_module: BlockPyModule<CodegenBlockPyPass>,
        module_name: String,
        package_name: String,
    ) -> PyResult<()> {
        if self.initialized {
            return Err(PyRuntimeError::new_err(
                "transformed module state was unexpectedly initialized twice",
            ));
        }
        let function_index_by_id = build_function_index_by_id(&lowered_module)?;
        let (counter_slots_by_id, counter_values, call_target_counter_values) =
            build_counter_storage(&lowered_module.counter_defs)?;
        let codegen_constants = ModuleCodegenConstants::collect_from_module(&lowered_module);
        let module_constant_objs = codegen_constants.build_python_constants(py)?;
        self.shared_state.write(Arc::new(SharedModuleState {
            lowered_module,
            module_name,
            package_name,
            codegen_constants,
            function_index_by_id,
            module_constant_objs,
            counter_slots_by_id,
            counter_values,
            call_target_counter_values,
            compiled_direct_runner_handles: Mutex::new(HashMap::new()),
        }));
        self.initialized = true;
        self.global_cache_initialized = false;
        Ok(())
    }

    unsafe fn clear(&mut self) {
        if !self.initialized {
            return;
        }
        let shared_state = unsafe { self.shared_state.assume_init_ref().as_ref() };
        if let Some(path) = counter_dump_file_from_env() {
            if let Err(err) = shared_state.append_counter_dump_file(path.as_path()) {
                eprintln!(
                    "[soac counters] failed to append counter dump to {}: {err}",
                    path.display()
                );
            }
        }
        if self.global_cache_initialized {
            unsafe { ptr::drop_in_place(self.global_cache.as_mut_ptr()) };
            self.global_cache_initialized = false;
        }
        unsafe { ptr::drop_in_place(self.shared_state.as_mut_ptr()) };
        self.initialized = false;
    }

    unsafe fn data(&self) -> PyResult<SoacExtModuleDataRef<'_>> {
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "missing transformed-module lowering data in module state",
            ));
        }
        Ok(SoacExtModuleDataRef {
            shared_state: unsafe { self.shared_state.assume_init_ref().as_ref() },
        })
    }

    unsafe fn clone_shared_state(&self) -> PyResult<Arc<SharedModuleState>> {
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "missing transformed-module lowering data in module state",
            ));
        }
        Ok(unsafe { self.shared_state.assume_init_ref().clone() })
    }

    unsafe fn clone_or_init_global_cache(
        &mut self,
        globals_obj: *mut ffi::PyObject,
    ) -> PyResult<Arc<ModuleGlobalCache>> {
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "missing transformed-module lowering data in module state",
            ));
        }
        if self.global_cache_initialized {
            return Ok(unsafe { self.global_cache.assume_init_ref().clone() });
        }
        let global_names = unsafe {
            self.shared_state
                .assume_init_ref()
                .lowered_module
                .global_names
                .clone()
        };
        let cache = unsafe { ModuleGlobalCache::new(globals_obj, global_names.as_slice()) }
            .map_err(|_| {
                if unsafe { ffi::PyErr_Occurred() }.is_null() {
                    PyRuntimeError::new_err("failed to create module global cache")
                } else {
                    PyErr::fetch(Python::assume_attached())
                }
            })?;
        self.global_cache.write(cache.clone());
        self.global_cache_initialized = true;
        Ok(cache)
    }
}

fn counter_dump_file_from_env() -> Option<std::path::PathBuf> {
    let raw = env::var("DIET_PYTHON_COUNTERS_OUTPUT_FILE").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.into())
    }
}

unsafe extern "C" fn soac_ext_module_clear(module: *mut ffi::PyObject) -> c_int {
    let state = unsafe { ffi::PyModule_GetState(module) }.cast::<SoacExtModuleState>();
    if state.is_null() {
        return 0;
    }
    unsafe { (*state).clear() };
    0
}

unsafe extern "C" fn soac_ext_module_traverse(
    module: *mut ffi::PyObject,
    visit: ffi::visitproc,
    arg: *mut c_void,
) -> c_int {
    let state = unsafe { ffi::PyModule_GetState(module) }.cast::<SoacExtModuleState>();
    if state.is_null() || unsafe { !(*state).initialized } {
        return 0;
    }
    let shared_state = unsafe { (*state).shared_state.assume_init_ref().as_ref() };
    for obj in &shared_state.module_constant_objs {
        let rc = unsafe { visit(obj.as_ptr(), arg) };
        if rc != 0 {
            return rc;
        }
    }
    0
}

unsafe extern "C" fn soac_ext_module_free(module: *mut c_void) {
    unsafe {
        soac_ext_module_clear(module.cast());
    }
}

static mut SOAC_EXT_MODULE_DEF: ffi::PyModuleDef = ffi::PyModuleDef {
    m_base: ffi::PyModuleDef_HEAD_INIT,
    m_name: c"_soac_ext.module_state".as_ptr(),
    m_doc: ptr::null(),
    m_size: std::mem::size_of::<SoacExtModuleState>() as ffi::Py_ssize_t,
    m_methods: ptr::null_mut(),
    m_slots: ptr::null_mut(),
    m_traverse: Some(soac_ext_module_traverse),
    m_clear: Some(soac_ext_module_clear),
    m_free: Some(soac_ext_module_free),
};

fn soac_ext_module_def() -> *mut ffi::PyModuleDef {
    ptr::addr_of_mut!(SOAC_EXT_MODULE_DEF)
}

fn soac_ext_module_state(module: &Bound<'_, PyAny>) -> PyResult<*mut SoacExtModuleState> {
    unsafe {
        let module_def = ffi::PyModule_GetDef(module.as_ptr());
        if module_def != soac_ext_module_def() {
            return Err(PyTypeError::new_err(
                "expected a module created via _soac_ext.create_module",
            ));
        }
        let state = ffi::PyModule_GetState(module.as_ptr()).cast::<SoacExtModuleState>();
        if state.is_null() {
            if ffi::PyErr_Occurred().is_null() {
                Err(PyRuntimeError::new_err(
                    "missing _soac_ext module state for transformed module",
                ))
            } else {
                Err(PyErr::fetch(module.py()))
            }
        } else {
            Ok(state)
        }
    }
}

pub struct SoacExtModule;

impl SoacExtModule {
    pub fn new(
        py: Python<'_>,
        spec: &Bound<'_, PyAny>,
        lowered_module: BlockPyModule<CodegenBlockPyPass>,
    ) -> PyResult<Py<PyAny>> {
        let module_name = spec
            .getattr("name")?
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err("expected a module spec with a string 'name'"))?;
        let package_name = spec
            .getattr("parent")?
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err("expected a module spec with a string 'parent'"))?;
        let module = unsafe {
            Bound::from_owned_ptr_or_err(
                py,
                ffi::PyModule_FromDefAndSpec(soac_ext_module_def(), spec.as_ptr()),
            )?
        };
        if unsafe { ffi::PyModule_ExecDef(module.as_ptr(), soac_ext_module_def()) } != 0 {
            return Err(PyErr::fetch(py));
        }
        let state = soac_ext_module_state(&module)?;
        unsafe {
            (*state).init(py, lowered_module, module_name, package_name)?;
        }
        Ok(module.unbind())
    }

    pub fn with_data<R>(
        module: &Bound<'_, PyAny>,
        f: impl FnOnce(SoacExtModuleDataRef<'_>) -> PyResult<R>,
    ) -> PyResult<R> {
        let state = soac_ext_module_state(module)?;
        unsafe { f((*state).data()?) }
    }

    pub fn clone_shared_state(module: &Bound<'_, PyAny>) -> PyResult<Arc<SharedModuleState>> {
        let state = soac_ext_module_state(module)?;
        unsafe { (*state).clone_shared_state() }
    }

    pub fn clone_or_init_global_cache(
        module: &Bound<'_, PyAny>,
        globals_obj: *mut ffi::PyObject,
    ) -> PyResult<Arc<ModuleGlobalCache>> {
        let state = soac_ext_module_state(module)?;
        unsafe { (*state).clone_or_init_global_cache(globals_obj) }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::counter_dump::COUNTER_DUMP_MAGIC;
    use soac_blockpy::lower_python_to_blockpy_for_testing;
    use soac_blockpy::passes::instrument_bb_module_with_block_entry_counters;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn counter_dump_record_includes_block_entry_metadata_and_value() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
def f():
    return None
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        instrument_bb_module_with_block_entry_counters(&mut lowered);

        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .expect("missing lowered function f");
        let function_id = function.function_id;
        let entry_label = function.entry_block().label;
        let entry_label_text = entry_label.to_string();

        let shared_state = SharedModuleState {
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            module_constant_objs: Vec::new(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0)].into_boxed_slice(),
            counter_values: vec![3].into_boxed_slice(),
            call_target_counter_values: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: String::new(),
            compiled_direct_runner_handles: Mutex::new(HashMap::new()),
        };

        let record = shared_state
            .counter_dump_record()
            .expect("counter dump record should be present");
        assert_eq!(record.module_name, "counter_test");
        let row = record
            .rows
            .iter()
            .find(|row| {
                row.kind == "block_entry"
                    && row.block_label.as_deref() == Some(entry_label_text.as_str())
            })
            .expect("entry block counter row should be present");
        assert_eq!(row.scope, "this");
        assert_eq!(row.kind, "block_entry");
        assert_eq!(row.site_kind, "block_entry");
        assert_eq!(row.function_id, Some(function_id));
        assert_eq!(row.current_function_id, Some(function_id));
        assert_eq!(row.function_qualname.as_deref(), Some("f"));
        assert_eq!(row.block_label, Some(entry_label_text));
        assert_eq!(row.value, 3);
        assert_eq!(row.observed_value, None);
        assert_eq!(row.max_overcount, None);
    }

    #[test]
    fn counter_scope_controls_storage_sharing() {
        let counter_defs = vec![
            CounterDef {
                id: CounterId(0),
                scope: CounterScope::Function,
                kind: "runtime_incref".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(FunctionId::new(0, 7)),
                    instr_id: None,
                },
            },
            CounterDef {
                id: CounterId(1),
                scope: CounterScope::Function,
                kind: "runtime_incref".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(FunctionId::new(0, 7)),
                    instr_id: None,
                },
            },
            CounterDef {
                id: CounterId(2),
                scope: CounterScope::Global,
                kind: "runtime_decref".to_string(),
                site: CounterSite::Runtime {
                    function_id: None,
                    instr_id: None,
                },
            },
            CounterDef {
                id: CounterId(3),
                scope: CounterScope::Global,
                kind: "runtime_decref".to_string(),
                site: CounterSite::Runtime {
                    function_id: None,
                    instr_id: None,
                },
            },
            CounterDef {
                id: CounterId(4),
                scope: CounterScope::This,
                kind: "block_entry".to_string(),
                site: CounterSite::BlockEntry {
                    function_id: FunctionId::new(0, 7),
                    block_label: soac_blockpy::block_py::BlockLabel::from_index(0),
                },
            },
        ];

        let (slots_by_id, counter_values, call_target_counter_values) =
            build_counter_storage(&counter_defs).expect("counter storage should build");
        assert_eq!(counter_values.len(), 3);
        assert!(call_target_counter_values.is_empty());
        assert_eq!(slots_by_id[0], slots_by_id[1]);
        assert_eq!(slots_by_id[2], slots_by_id[3]);
        assert_ne!(slots_by_id[0], slots_by_id[2]);
        assert_ne!(slots_by_id[0], slots_by_id[4]);
        assert_ne!(slots_by_id[2], slots_by_id[4]);
    }

    #[test]
    fn append_counter_dump_file_writes_binary_record() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
VALUE = 1

def f():
    return VALUE
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        instrument_bb_module_with_block_entry_counters(&mut lowered);

        let shared_state = SharedModuleState {
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            module_constant_objs: Vec::new(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0), CounterRuntimeSlot::Scalar(1)]
                .into_boxed_slice(),
            counter_values: vec![5, 8].into_boxed_slice(),
            call_target_counter_values: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: "pkg".to_string(),
            compiled_direct_runner_handles: Mutex::new(HashMap::new()),
        };

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "soac_counter_dump_module_type_{unique}_{}.bin",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_file(&path).expect("stale temp file should be removable");
        }

        shared_state
            .append_counter_dump_file(path.as_path())
            .expect("counter dump file should be written");

        let bytes = fs::read(&path).expect("counter dump file should be readable");
        assert!(bytes.starts_with(COUNTER_DUMP_MAGIC.as_slice()));
        assert!(!bytes.is_empty());

        fs::remove_file(&path).expect("temp file should be removable");
    }
}
