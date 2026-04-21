use crate::config::SpecializationMode;
use crate::counter::{CounterEntry, GilTopValueCounter, TopValueCounter};
use crate::jit::JitCodegenStats;
use crate::module_constants::{ModuleCodegenConstants, load_runtime_name_owned_by_id};
use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyAnyMethods, PyList, PyTuple};
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, CounterDef, CounterId, CounterScope, CounterSite,
    DeoptEntrySource, FunctionExecutionMode, RuntimeFunctionId, RuntimeName,
};
use soac_core::profile::{
    CounterDumpKeyLayout, CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey,
    CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry,
};
use soac_driver::codegen_cache::PythonModuleCacheSource;
use soac_lowering::passes::{
    CodegenModuleShape, InlinePlanModule, plan_module_inlining,
    specialization_runtime_logging_enabled, summarize_module_escapes,
};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_int, c_void};
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::mem::MaybeUninit;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::info;

unsafe extern "C" {
    fn _PyDict_WatchSplitKeysForType(type_obj: *mut ffi::PyObject) -> c_int;
    fn _PyDict_GetKeyLayoutEvents() -> *mut ffi::PyObject;
    fn _PyDict_NewIndexedKeySet(keys: *mut ffi::PyObject) -> *mut c_void;
    fn _PyDict_NewWithIndexedKeySet(keys: *mut c_void) -> *mut ffi::PyObject;
    fn _PyDictKeys_DecRef(keys: *mut c_void);
}

pub struct SoacExtModuleDataRef<'a> {
    pub shared_state: &'a SharedModuleState,
}

#[repr(C)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleInfo {
    pub hash: u64,
    pub cache_source: Option<PythonModuleCacheSource>,
    pub indexed_module_keys: Vec<String>,
}

pub fn hash_module_source(source: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

pub struct SharedModuleState {
    pub lowered_module: BlockPyModule<CodegenModuleShape>,
    pub inline_plan: InlinePlanModule,
    pub module_name: String,
    pub package_name: String,
    pub source_hash: u64,
    pub module_cache_source: Option<PythonModuleCacheSource>,
    pub codegen_constants: ModuleCodegenConstants,
    storage_instance_key: usize,
    function_index_by_id: HashMap<RuntimeFunctionId, usize>,
    original_code_by_function_id: HashMap<RuntimeFunctionId, Py<PyAny>>,
    module_constant_objs: Vec<Py<PyAny>>,
    // Each non-null slot owns one runtime-name reference for this module state; lookups return a
    // fresh owned reference by INCREFing the cached pointer.
    runtime_name_cache: Box<[AtomicUsize]>,
    counter_slots_by_id: Box<[CounterRuntimeSlot]>,
    counter_values: Box<[u64]>,
    top_value_counters: Box<[GilTopValueCounter]>,
    pub(crate) precompiled_module_runtime:
        OnceLock<Result<Arc<crate::jit::PrecompiledModuleRuntime>, String>>,
    pub(crate) jit_module_plan: OnceLock<Result<Arc<crate::jit::JitModulePlan>, String>>,
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
pub(crate) enum CounterRuntimeSlot {
    Scalar(usize),
    TopValues(usize),
}

fn build_runtime_name_cache() -> Box<[AtomicUsize]> {
    RuntimeName::ALL
        .iter()
        .map(|_| AtomicUsize::new(0))
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

impl SharedModuleState {
    pub(crate) unsafe fn runtime_name_owned_cached(
        &self,
        runtime_name: RuntimeName,
    ) -> *mut ffi::PyObject {
        let Some(slot) = self.runtime_name_cache.get(usize::from(runtime_name.id())) else {
            return unsafe { load_runtime_name_owned_by_id(runtime_name) };
        };
        let cached = slot.load(Ordering::Acquire) as *mut ffi::PyObject;
        if !cached.is_null() {
            unsafe {
                ffi::Py_INCREF(cached);
            }
            return cached;
        }

        let loaded = unsafe { load_runtime_name_owned_by_id(runtime_name) };
        if loaded.is_null() {
            return loaded;
        }
        match slot.compare_exchange(0, loaded as usize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                unsafe {
                    ffi::Py_INCREF(loaded);
                }
                loaded
            }
            Err(existing) => {
                unsafe {
                    ffi::Py_DECREF(loaded);
                }
                let existing = existing as *mut ffi::PyObject;
                if !existing.is_null() {
                    unsafe {
                        ffi::Py_INCREF(existing);
                    }
                }
                existing
            }
        }
    }

    pub(crate) fn storage_instance_key(&self) -> usize {
        self.storage_instance_key
    }

    pub fn module_id(&self) -> u32 {
        self.lowered_module.module_name_gen.module_id()
    }

    pub fn source_hash(&self) -> u64 {
        self.source_hash
    }

    pub fn lookup_function(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<&BlockPyFunction<CodegenModuleShape>> {
        let function_index = self.function_index_by_id.get(&function_id).copied()?;
        let function = self.lowered_module.callable_defs.get(function_index)?;
        assert_eq!(function.function_id, function_id);
        Some(function)
    }

    pub(crate) fn lookup_direct_call_target_function(
        &self,
        compile_session: &crate::session::CompileSession,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<BlockPyFunction<CodegenModuleShape>>, String> {
        if function_id == RuntimeFunctionId::global() {
            return Ok(None);
        }
        if let Some(function) = self.lookup_function(function_id) {
            return Ok(Some(function.clone()));
        }
        if function_id.runtime_module_id().as_u32() == self.module_id() {
            return Ok(None);
        }
        Ok(compile_session
            .lookup_shared_function(function_id)?
            .map(|(_shared_state, function)| function))
    }

    pub fn lookup_original_code(&self, function_id: RuntimeFunctionId) -> Option<&Py<PyAny>> {
        self.original_code_by_function_id.get(&function_id)
    }

    pub(crate) fn module_constant_ptrs(&self) -> Vec<*mut ffi::PyObject> {
        self.module_constant_objs
            .iter()
            .map(|obj| obj.as_ptr())
            .collect()
    }

    pub fn module_constant_obj(
        &self,
        id: crate::module_constants::ModuleConstantId,
    ) -> Option<&Py<PyAny>> {
        self.module_constant_objs.get(id.0)
    }

    pub(crate) fn counter_slots_by_id(&self) -> &[CounterRuntimeSlot] {
        &self.counter_slots_by_id
    }

    pub(crate) fn scalar_counter_values_ptr(&self) -> *mut u64 {
        if self.counter_values.is_empty() {
            ptr::null_mut()
        } else {
            self.counter_values.as_ptr() as *mut u64
        }
    }

    pub(crate) fn top_value_counter_values_ptr(&self) -> *mut TopValueCounter {
        self.top_value_counters
            .first()
            .map(GilTopValueCounter::as_raw_ptr)
            .unwrap_or(ptr::null_mut())
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
            CounterRuntimeSlot::TopValues(_) => 0,
        }
    }

    fn top_values_counter_snapshot(&self, counter_id: CounterId) -> Option<Vec<CounterEntry<u64>>> {
        let CounterRuntimeSlot::TopValues(slot) =
            self.counter_slots_by_id.get(counter_id.0).copied()?
        else {
            return None;
        };
        let counter = self.top_value_counters.get(slot)?;
        Some(unsafe { counter.snapshot_with_gil() })
    }

    pub(crate) fn lookup_or_compile_direct_function_handle(
        &self,
        compile_session: &Arc<crate::session::CompileSession>,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<(Arc<crate::jit::CompiledFunctionHandle>, bool)>, String> {
        if function_id == RuntimeFunctionId::global() {
            return Ok(None);
        }
        if function_id != RuntimeFunctionId::global()
            && function_id.runtime_module_id().as_u32() != self.module_id()
        {
            let Some((shared_state, _function)) =
                compile_session.lookup_shared_function(function_id)?
            else {
                return Ok(None);
            };
            return shared_state
                .lookup_or_compile_direct_function_handle(compile_session, function_id);
        }
        if crate::jit::process_jit_is_currently_compiling() {
            return Ok(None);
        }
        let function = self
            .lookup_function(function_id)
            .cloned()
            .ok_or_else(|| format!("missing direct-call target for function_id={function_id}"))?;
        if function.execution_mode() == FunctionExecutionMode::Interpreted {
            return Ok(None);
        }
        if let Some(handle) =
            crate::jit::lookup_precompiled_direct_function_handle(compile_session, self, &function)?
        {
            return Ok(Some((handle, false)));
        }
        let blocks = vec![ptr::null_mut::<c_void>(); function.blocks.len()];
        let module_constant_ptrs = self.module_constant_ptrs();
        let compile_start = std::time::Instant::now();
        let compile_result = unsafe {
            crate::jit::compile_cranelift_run_bb_specialized_cached(
                compile_session,
                blocks.as_slice(),
                &self.lowered_module,
                &function,
                &self.codegen_constants,
                &self.lowered_module.counter_defs,
                &module_constant_ptrs,
                Some(self),
            )
        };
        match compile_result {
            Ok(result) => {
                if result.compiled {
                    self.append_jit_codegen_log(
                        &function,
                        "direct_function_body",
                        compile_start.elapsed(),
                        "ok",
                        None,
                        result.stats.as_ref(),
                    );
                }
                Ok(Some((result.handle, result.compiled)))
            }
            Err(err) => {
                self.append_jit_codegen_log(
                    &function,
                    "direct_function_body",
                    compile_start.elapsed(),
                    "error",
                    Some(&err),
                    None,
                );
                return Err(format!(
                    "{err} [direct_target={} id={}]",
                    function.names.qualname, function.function_id
                ));
            }
        }
    }

    pub(crate) fn append_jit_codegen_log(
        &self,
        function: &BlockPyFunction<CodegenModuleShape>,
        entry_kind: &str,
        elapsed: Duration,
        status: &str,
        error: Option<&str>,
        stats: Option<&JitCodegenStats>,
    ) {
        append_jit_codegen_log(self, function, entry_kind, elapsed, status, error, stats);
    }

    pub fn append_specialization_runtime_log(&self) {
        let env_config = match SoacEnvConfig::from_env() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("[soac counters] invalid specialization runtime log config: {err}");
                return;
            }
        };
        if !specialization_runtime_logging_enabled(&env_config) {
            return;
        }
        for counter in &self.lowered_module.counter_defs {
            let kind = counter.kind.as_str();
            if !matches!(
                kind,
                "global_indexed_hit"
                    | "global_indexed_fallback"
                    | "field_indexed_hit"
                    | "field_indexed_fallback"
                    | "operator_specialized_hit"
                    | "operator_specialized_fallback"
                    | "getitem_specialized_hit"
                    | "getitem_specialized_fallback"
                    | "setitem_specialized_hit"
                    | "setitem_specialized_fallback"
                    | "call_direct_hit"
                    | "call_direct_fallback"
                    | "deopt_entry_guard_miss"
            ) {
                continue;
            }
            let value = self.counter_value(counter.id);
            if value == 0 {
                continue;
            }
            let (function_id, instr_id, function_qualname, block_label) = match &counter.site {
                CounterSite::BlockEntry { .. } => {
                    (String::new(), String::new(), String::new(), String::new())
                }
                CounterSite::DeoptEntry {
                    function_id,
                    source,
                } => (
                    function_id.to_string(),
                    deopt_entry_source_instr_id(*source)
                        .map(|instr_id| instr_id.to_string())
                        .unwrap_or_default(),
                    self.lookup_function(*function_id)
                        .map(|function| function.names.qualname.clone())
                        .unwrap_or_default(),
                    deopt_entry_source_block_label(*source),
                ),
                CounterSite::Runtime {
                    function_id,
                    instr_id,
                } => (
                    function_id
                        .map(|function_id| function_id.to_string())
                        .unwrap_or_default(),
                    instr_id
                        .map(|instr_id| instr_id.to_string())
                        .unwrap_or_default(),
                    function_id
                        .and_then(|function_id| {
                            self.lookup_function(function_id)
                                .map(|function| function.names.qualname.clone())
                        })
                        .unwrap_or_default(),
                    String::new(),
                ),
            };
            info!(
                target: "soac_specialization_runtime",
                event = "soac.specialization_runtime",
                module_name = self.module_name,
                package_name = self.package_name,
                kind,
                scope = counter_scope_name(counter.scope),
                function_id,
                function_qualname,
                instr_id,
                block_label,
                value,
                "specialization_runtime",
            );
        }
    }

    pub fn counter_dump_record(&self) -> Option<CounterDumpRecord> {
        let module_keys = self.counter_dump_module_keys();
        let (type_keys, type_table) = self.counter_dump_type_key_layouts();

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
                CounterSite::DeoptEntry {
                    function_id,
                    source,
                } => (
                    "deopt_entry".to_string(),
                    Some(*function_id),
                    Some(*function_id),
                    deopt_entry_source_instr_id(*source),
                    self.lookup_function(*function_id)
                        .map(|function| function.names.qualname.clone()),
                    Some(deopt_entry_source_block_label(*source)),
                ),
                CounterSite::Runtime {
                    function_id,
                    instr_id,
                } => (
                    "runtime".to_string(),
                    Some(function_id.unwrap_or(RuntimeFunctionId::global())),
                    Some(function_id.unwrap_or(RuntimeFunctionId::global())),
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
                    .top_values_counter_snapshot(counter.id)
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

        if rows.is_empty()
            && module_keys.is_empty()
            && type_keys.is_empty()
            && type_table.is_empty()
        {
            return None;
        }

        Some(CounterDumpRecord {
            source_hash: self.source_hash,
            module_name: self.module_name.clone(),
            package_name: (!self.package_name.is_empty()).then(|| self.package_name.clone()),
            rows,
            module_keys,
            type_keys,
            type_table,
        })
    }

    fn counter_dump_module_keys(&self) -> Vec<CounterDumpKeyLayout> {
        if !key_layout_counter_enabled() {
            return Vec::new();
        }
        self.lowered_module
            .global_names
            .iter()
            .enumerate()
            .map(|(index, key)| CounterDumpKeyLayout {
                owner: self.module_name.clone(),
                key: key.clone(),
                index: u32::try_from(index).expect("global-name index should fit in u32"),
            })
            .collect()
    }

    fn counter_dump_type_key_layouts(
        &self,
    ) -> (
        Vec<CounterDumpTypeKeyLayout>,
        Vec<CounterDumpTypeTableEntry>,
    ) {
        if !key_layout_counter_enabled() {
            return (Vec::new(), Vec::new());
        }
        snapshot_type_key_layout_events()
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

static NEXT_SHARED_MODULE_STATE_STORAGE_KEY: AtomicUsize = AtomicUsize::new(1);

fn allocate_shared_module_state_storage_key() -> usize {
    NEXT_SHARED_MODULE_STATE_STORAGE_KEY.fetch_add(1, Ordering::Relaxed)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) unsafe fn record_top_value_sample_counter_ptr(
    counter: *mut c_void,
    value: u64,
) -> Result<(), String> {
    if counter.is_null() {
        return Err("missing direct top-value counter pointer".to_string());
    }
    let counter = unsafe { &mut *(counter as *mut TopValueCounter) };
    counter.record(value);
    Ok(())
}

fn counter_scope_name(scope: CounterScope) -> &'static str {
    match scope {
        CounterScope::This => "this",
        CounterScope::Function => "function",
        CounterScope::Global => "global",
    }
}

fn deopt_entry_source_instr_id(source: DeoptEntrySource) -> Option<soac_core::block_py::InstrId> {
    match source {
        DeoptEntrySource::BeforeInstr { instr_id } => Some(instr_id),
        DeoptEntrySource::BlockEntry { .. } | DeoptEntrySource::BeforeTerm { .. } => None,
    }
}

fn deopt_entry_source_block_label(source: DeoptEntrySource) -> String {
    match source {
        DeoptEntrySource::BlockEntry { block_label }
        | DeoptEntrySource::BeforeTerm { block_label } => block_label.to_string(),
        DeoptEntrySource::BeforeInstr { instr_id } => instr_id.block_label().to_string(),
    }
}

fn counter_storage_key(counter: &CounterDef) -> Result<CounterStorageKey, String> {
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
        "branch_outcomes"
            | "call_hot_targets"
            | "operator_hot_shapes"
            | "getitem_hot_shapes"
            | "setitem_hot_shapes"
    )
}

pub(crate) fn build_counter_storage_layout(
    counter_defs: &[CounterDef],
) -> Result<(Box<[CounterRuntimeSlot]>, usize, usize), String> {
    let mut slots_by_id = vec![None; counter_defs.len()];
    let mut scalar_slot_by_key = HashMap::new();
    let mut top_values_slot_by_key = HashMap::new();
    for counter in counter_defs {
        if counter.id.0 >= slots_by_id.len() {
            return Err(format!(
                "counter id {} is out of range for {} counter defs",
                counter.id.0,
                counter_defs.len()
            ));
        }
        let key = counter_storage_key(counter)?;
        let slot = if counter_uses_call_target_storage(counter) {
            let slot = if let Some(slot) = top_values_slot_by_key.get(&key).copied() {
                slot
            } else {
                let slot = top_values_slot_by_key.len();
                top_values_slot_by_key.insert(key, slot);
                slot
            };
            CounterRuntimeSlot::TopValues(slot)
        } else {
            let slot = if let Some(slot) = scalar_slot_by_key.get(&key).copied() {
                slot
            } else {
                let slot = scalar_slot_by_key.len();
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
        scalar_slot_by_key.len(),
        top_values_slot_by_key.len(),
    ))
}

fn build_counter_storage(
    counter_defs: &[CounterDef],
) -> PyResult<(
    Box<[CounterRuntimeSlot]>,
    Box<[u64]>,
    Box<[GilTopValueCounter]>,
)> {
    let (slots_by_id, scalar_count, top_value_count) =
        build_counter_storage_layout(counter_defs).map_err(PyRuntimeError::new_err)?;
    Ok((
        slots_by_id,
        vec![0; scalar_count].into_boxed_slice(),
        (0..top_value_count)
            .map(|_| GilTopValueCounter::new())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    ))
}

pub(crate) fn build_module_constant_objects(
    py: Python<'_>,
    codegen_constants: &ModuleCodegenConstants,
    module_name: &str,
    source_hash: u64,
) -> PyResult<Vec<Py<PyAny>>> {
    codegen_constants.build_python_constants_with_static_resolver(
        py,
        module_name == "soac.runtime",
        |constant_id| {
            crate::jit::lookup_precompiled_static_module_constant(
                module_name,
                source_hash,
                constant_id,
            )
            .map_err(PyRuntimeError::new_err)
        },
    )
}

pub fn build_shared_state_for_inspection(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
) -> PyResult<Arc<SharedModuleState>> {
    build_shared_state_for_inspection_with_original_code(
        py,
        lowered_module,
        module_name,
        package_name,
        HashMap::new(),
    )
}

pub fn build_shared_state_for_inspection_with_source_hash(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
    source_hash: u64,
) -> PyResult<Arc<SharedModuleState>> {
    build_shared_state_for_inspection_with_original_code_and_source_hash(
        py,
        lowered_module,
        module_name,
        package_name,
        source_hash,
        HashMap::new(),
    )
}

pub fn build_shared_state_for_inspection_with_placeholder_constants(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
) -> PyResult<Arc<SharedModuleState>> {
    let function_index_by_id = build_function_index_by_id(&lowered_module)?;
    let (counter_slots_by_id, counter_values, top_value_counters) =
        build_counter_storage(&lowered_module.counter_defs)?;
    let codegen_constants = if module_name == "soac.runtime" {
        ModuleCodegenConstants::collect_from_runtime_module(&lowered_module)
    } else {
        ModuleCodegenConstants::collect_from_module(&lowered_module)
    };
    let module_constant_objs = (0..codegen_constants.len())
        .map(|_| py.None())
        .collect::<Vec<_>>();
    let inline_plan = plan_inline_candidates(&lowered_module);
    Ok(Arc::new(SharedModuleState {
        lowered_module,
        inline_plan,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        source_hash: 0,
        module_cache_source: None,
        codegen_constants,
        storage_instance_key: allocate_shared_module_state_storage_key(),
        function_index_by_id,
        original_code_by_function_id: HashMap::new(),
        module_constant_objs,
        runtime_name_cache: build_runtime_name_cache(),
        counter_slots_by_id,
        counter_values,
        top_value_counters,
        precompiled_module_runtime: OnceLock::new(),
        jit_module_plan: OnceLock::new(),
    }))
}

pub fn build_shared_state_for_inspection_with_placeholder_constants_and_source_hash(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
    source_hash: u64,
) -> PyResult<Arc<SharedModuleState>> {
    let function_index_by_id = build_function_index_by_id(&lowered_module)?;
    let (counter_slots_by_id, counter_values, top_value_counters) =
        build_counter_storage(&lowered_module.counter_defs)?;
    let codegen_constants = if module_name == "soac.runtime" {
        ModuleCodegenConstants::collect_from_runtime_module(&lowered_module)
    } else {
        ModuleCodegenConstants::collect_from_module(&lowered_module)
    };
    let module_constant_objs = (0..codegen_constants.len())
        .map(|_| py.None())
        .collect::<Vec<_>>();
    let inline_plan = plan_inline_candidates(&lowered_module);
    Ok(Arc::new(SharedModuleState {
        lowered_module,
        inline_plan,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        source_hash,
        module_cache_source: None,
        codegen_constants,
        storage_instance_key: allocate_shared_module_state_storage_key(),
        function_index_by_id,
        original_code_by_function_id: HashMap::new(),
        module_constant_objs,
        runtime_name_cache: build_runtime_name_cache(),
        counter_slots_by_id,
        counter_values,
        top_value_counters,
        precompiled_module_runtime: OnceLock::new(),
        jit_module_plan: OnceLock::new(),
    }))
}

#[cfg(test)]
pub(crate) fn build_shared_state_for_testing_with_original_code(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
    original_code_by_function_id: HashMap<RuntimeFunctionId, Py<PyAny>>,
) -> PyResult<Arc<SharedModuleState>> {
    build_shared_state_for_inspection_with_original_code(
        py,
        lowered_module,
        module_name,
        package_name,
        original_code_by_function_id,
    )
}

fn build_shared_state_for_inspection_with_original_code(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
    original_code_by_function_id: HashMap<RuntimeFunctionId, Py<PyAny>>,
) -> PyResult<Arc<SharedModuleState>> {
    build_shared_state_for_inspection_with_original_code_and_source_hash(
        py,
        lowered_module,
        module_name,
        package_name,
        0,
        original_code_by_function_id,
    )
}

fn build_shared_state_for_inspection_with_original_code_and_source_hash(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
    source_hash: u64,
    original_code_by_function_id: HashMap<RuntimeFunctionId, Py<PyAny>>,
) -> PyResult<Arc<SharedModuleState>> {
    let function_index_by_id = build_function_index_by_id(&lowered_module)?;
    let (counter_slots_by_id, counter_values, top_value_counters) =
        build_counter_storage(&lowered_module.counter_defs)?;
    let codegen_constants = if module_name == "soac.runtime" {
        ModuleCodegenConstants::collect_from_runtime_module(&lowered_module)
    } else {
        ModuleCodegenConstants::collect_from_module(&lowered_module)
    };
    let module_constant_objs =
        build_module_constant_objects(py, &codegen_constants, module_name, source_hash)?;
    let inline_plan = plan_inline_candidates(&lowered_module);
    Ok(Arc::new(SharedModuleState {
        lowered_module,
        inline_plan,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        source_hash,
        module_cache_source: None,
        codegen_constants,
        storage_instance_key: allocate_shared_module_state_storage_key(),
        function_index_by_id,
        original_code_by_function_id,
        module_constant_objs,
        runtime_name_cache: build_runtime_name_cache(),
        counter_slots_by_id,
        counter_values,
        top_value_counters,
        precompiled_module_runtime: OnceLock::new(),
        jit_module_plan: OnceLock::new(),
    }))
}

#[cfg(test)]
pub(crate) fn build_shared_state_for_testing(
    py: Python<'_>,
    lowered_module: BlockPyModule<CodegenModuleShape>,
    module_name: &str,
    package_name: &str,
) -> PyResult<Arc<SharedModuleState>> {
    build_shared_state_for_inspection(py, lowered_module, module_name, package_name)
}

fn build_function_index_by_id(
    module: &BlockPyModule<CodegenModuleShape>,
) -> PyResult<HashMap<RuntimeFunctionId, usize>> {
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

fn plan_inline_candidates(module: &BlockPyModule<CodegenModuleShape>) -> InlinePlanModule {
    let escape_summary = summarize_module_escapes(module);
    plan_module_inlining(&escape_summary)
}

#[repr(C)]
struct SoacExtModuleState {
    initialized: bool,
    shared_state: MaybeUninit<Arc<SharedModuleState>>,
}

impl SoacExtModuleState {
    unsafe fn init(
        &mut self,
        py: Python<'_>,
        compile_session: &Arc<crate::session::CompileSession>,
        lowered_module: BlockPyModule<CodegenModuleShape>,
        original_code_by_function_id: HashMap<RuntimeFunctionId, Py<PyAny>>,
        module_name: String,
        package_name: String,
        source_hash: u64,
        module_cache_source: Option<PythonModuleCacheSource>,
    ) -> PyResult<()> {
        if self.initialized {
            return Err(PyRuntimeError::new_err(
                "transformed module state was unexpectedly initialized twice",
            ));
        }
        let function_index_by_id = build_function_index_by_id(&lowered_module)?;
        let (counter_slots_by_id, counter_values, top_value_counters) =
            build_counter_storage(&lowered_module.counter_defs)?;
        let codegen_constants = if module_name == "soac.runtime" {
            ModuleCodegenConstants::collect_from_runtime_module(&lowered_module)
        } else {
            ModuleCodegenConstants::collect_from_module(&lowered_module)
        };
        let module_constant_objs = build_module_constant_objects(
            py,
            &codegen_constants,
            module_name.as_str(),
            source_hash,
        )?;
        let inline_plan = plan_inline_candidates(&lowered_module);
        let shared_state = Arc::new(SharedModuleState {
            lowered_module,
            inline_plan,
            module_name,
            package_name,
            source_hash,
            module_cache_source,
            codegen_constants,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            function_index_by_id,
            original_code_by_function_id,
            module_constant_objs,
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id,
            counter_values,
            top_value_counters,
            precompiled_module_runtime: OnceLock::new(),
            jit_module_plan: OnceLock::new(),
        });
        compile_session
            .retain_shared_module_state(shared_state.clone())
            .map_err(PyRuntimeError::new_err)?;
        self.shared_state.write(shared_state);
        self.initialized = true;
        Ok(())
    }

    unsafe fn clear(&mut self) {
        if !self.initialized {
            return;
        }
        let shared_state = unsafe { self.shared_state.assume_init_ref().as_ref() };
        shared_state.append_specialization_runtime_log();
        if let Some(path) = counter_dump_file_from_env() {
            if let Err(err) = shared_state.append_counter_dump_file(path.as_path()) {
                eprintln!(
                    "[soac counters] failed to append counter dump to {}: {err}",
                    path.display()
                );
            }
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
}

pub fn key_layout_counter_enabled() -> bool {
    specialization_mode_records_counters()
}

fn specialization_mode_records_counters() -> bool {
    match SoacEnvConfig::from_env() {
        Ok(config) => config
            .specialization_mode()
            .is_some_and(SpecializationMode::records_counters),
        Err(err) => {
            eprintln!("[soac counters] invalid specialization config: {err}");
            false
        }
    }
}

fn append_jit_codegen_log(
    module_state: &SharedModuleState,
    function: &BlockPyFunction<CodegenModuleShape>,
    entry_kind: &str,
    elapsed: Duration,
    status: &str,
    error: Option<&str>,
    stats: Option<&JitCodegenStats>,
) {
    let stats = stats.copied().unwrap_or_default();
    info!(
        target: "soac_jit_codegen",
        event = "soac.jit_codegen",
        status,
        error = error.unwrap_or(""),
        module_name = module_state.module_name,
        package_name = module_state.package_name,
        function_id = function.function_id.to_string(),
        function_qualname = function.names.qualname,
        function_block_count = function.blocks.len(),
        function_entry_kind = entry_kind,
        jit_codegen_total_us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
        jit_clif_block_count = u64::try_from(stats.clif_block_count).unwrap_or(u64::MAX),
        jit_clif_inst_count = u64::try_from(stats.clif_inst_count).unwrap_or(u64::MAX),
        jit_machine_code_size_bytes = u64::try_from(stats.machine_code_size_bytes).unwrap_or(u64::MAX),
        jit_machine_code_block_count = u64::try_from(stats.machine_code_block_count).unwrap_or(u64::MAX),
        jit_machine_code_edge_count = u64::try_from(stats.machine_code_edge_count).unwrap_or(u64::MAX),
        "jit_codegen",
    );
}

pub unsafe fn watch_split_keys_for_type(type_obj: *mut ffi::PyObject) -> Result<(), ()> {
    if !key_layout_counter_enabled() {
        return Ok(());
    }
    if type_obj.is_null() {
        return Err(());
    }
    if unsafe { _PyDict_WatchSplitKeysForType(type_obj) } == 0 {
        return Ok(());
    }
    if unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) } != 0 {
        unsafe { ffi::PyErr_Clear() };
        return Ok(());
    }
    Err(())
}

#[derive(Default)]
struct ProfileTypeRegistry {
    next_id: u64,
    by_type: HashMap<usize, u64>,
    entries: Vec<CounterDumpTypeTableEntry>,
}

impl ProfileTypeRegistry {
    fn id_for_type(&mut self, owner_ptr: usize, key: CounterDumpTypeKey) -> PyResult<u64> {
        if let Some(type_id) = self.by_type.get(&owner_ptr).copied() {
            return Ok(type_id);
        }
        let type_id = self.next_id.max(1);
        self.next_id = type_id
            .checked_add(1)
            .ok_or_else(|| PyRuntimeError::new_err("profile type id space exhausted"))?;
        self.by_type.insert(owner_ptr, type_id);
        self.entries
            .push(CounterDumpTypeTableEntry { type_id, key });
        Ok(type_id)
    }

    fn entries_for_ids(&self, used_ids: &HashSet<u64>) -> Vec<CounterDumpTypeTableEntry> {
        self.entries
            .iter()
            .filter(|entry| used_ids.contains(&entry.type_id))
            .cloned()
            .collect()
    }
}

static PROFILE_TYPE_REGISTRY: OnceLock<Mutex<ProfileTypeRegistry>> = OnceLock::new();

fn profile_type_registry() -> &'static Mutex<ProfileTypeRegistry> {
    PROFILE_TYPE_REGISTRY.get_or_init(|| Mutex::new(ProfileTypeRegistry::default()))
}

fn snapshot_type_key_layout_events() -> (
    Vec<CounterDumpTypeKeyLayout>,
    Vec<CounterDumpTypeTableEntry>,
) {
    let events = unsafe { _PyDict_GetKeyLayoutEvents() };
    if events.is_null() {
        unsafe { ffi::PyErr_Clear() };
        return (Vec::new(), Vec::new());
    }

    let py = unsafe { Python::assume_attached() };
    let events = unsafe { Bound::from_owned_ptr(py, events) };
    snapshot_type_key_layout_events_bound(events.as_any()).unwrap_or_default()
}

fn snapshot_type_key_layout_events_bound(
    events: &Bound<'_, PyAny>,
) -> PyResult<(
    Vec<CounterDumpTypeKeyLayout>,
    Vec<CounterDumpTypeTableEntry>,
)> {
    let events = events.cast::<PyList>()?;
    let mut out = Vec::new();
    let mut used_type_ids = HashSet::new();
    for event in events.iter() {
        let event = event.cast::<PyTuple>()?;
        let owner = event.get_item(0)?;
        let key: String = event.get_item(1)?.extract()?;
        let index: u32 = event.get_item(2)?.extract()?;
        let owner_ptr = owner.as_ptr() as usize;
        let type_key = CounterDumpTypeKey {
            module_name: owner.getattr("__module__")?.extract()?,
            qualname: owner.getattr("__qualname__")?.extract()?,
        };
        let owner_type_id = {
            let mut registry = profile_type_registry()
                .lock()
                .map_err(|_| PyRuntimeError::new_err("profile type registry lock was poisoned"))?;
            registry.id_for_type(owner_ptr, type_key)?
        };
        used_type_ids.insert(owner_type_id);
        out.push(CounterDumpTypeKeyLayout {
            owner_type_id,
            key,
            index,
        });
    }
    let type_table = profile_type_registry()
        .lock()
        .map_err(|_| PyRuntimeError::new_err("profile type registry lock was poisoned"))?
        .entries_for_ids(&used_type_ids);
    Ok((out, type_table))
}

fn counter_dump_file_from_env() -> Option<std::path::PathBuf> {
    let path = match SoacEnvConfig::from_env().map(|config| config.counter_dump_output_path()) {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(err) => {
            eprintln!("[soac counters] invalid counter dump config: {err}");
            return None;
        }
    };
    let Some(dir) = path.parent() else {
        return Some(path);
    };
    if let Err(err) = create_dir_all(dir) {
        eprintln!(
            "[soac counters] failed to create SOAC_WORK_DIR {}: {err}",
            dir.display()
        );
        return None;
    }
    Some(path)
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
    for code in shared_state.original_code_by_function_id.values() {
        let rc = unsafe { visit(code.as_ptr(), arg) };
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

unsafe fn soac_indexed_module_dict(keys: *mut ffi::PyObject) -> *mut ffi::PyObject {
    let indexed_keys = unsafe { _PyDict_NewIndexedKeySet(keys) };
    if indexed_keys.is_null() {
        return ptr::null_mut();
    }
    let indexed_dict = unsafe { _PyDict_NewWithIndexedKeySet(indexed_keys) };
    unsafe { _PyDictKeys_DecRef(indexed_keys) };
    indexed_dict
}

unsafe fn soac_module_dict_slot(module: *mut ffi::PyObject) -> PyResult<*mut *mut ffi::PyObject> {
    let type_obj = unsafe { ffi::Py_TYPE(module) };
    let offset = unsafe { (*type_obj).tp_dictoffset };
    if offset <= 0 {
        return Err(PyRuntimeError::new_err(
            "IndexedModuleType did not inherit a module __dict__ slot",
        ));
    }
    Ok(unsafe {
        module
            .cast::<u8>()
            .offset(offset as isize)
            .cast::<*mut ffi::PyObject>()
    })
}

fn soac_indexed_module_info_offset() -> usize {
    let base_size = unsafe { ffi::PyModule_Type.tp_basicsize as usize };
    let align = std::mem::align_of::<*mut ModuleInfo>();
    base_size.next_multiple_of(align)
}

unsafe fn soac_indexed_module_info_slot(module: *mut ffi::PyObject) -> *mut *mut ModuleInfo {
    unsafe {
        module
            .cast::<u8>()
            .add(soac_indexed_module_info_offset())
            .cast::<*mut ModuleInfo>()
    }
}

pub fn indexed_module_info(module: &Bound<'_, PyAny>) -> PyResult<ModuleInfo> {
    if !module.is_instance(soac_indexed_module_type(module.py())?)? {
        return Err(PyTypeError::new_err(
            "expected an instance of _soac_ext.IndexedModuleType",
        ));
    }
    let module_info = unsafe { *soac_indexed_module_info_slot(module.as_ptr()) };
    if module_info.is_null() {
        return Err(PyRuntimeError::new_err(
            "expected a transformed module initialized via _soac_ext.create_module",
        ));
    }
    Ok(unsafe { (*module_info).clone() })
}

unsafe fn soac_replace_module_dict(
    py: Python<'_>,
    module: *mut ffi::PyObject,
    indexed_dict: *mut ffi::PyObject,
) -> PyResult<()> {
    let old_dict = unsafe { ffi::PyModule_GetDict(module) };
    if old_dict.is_null() {
        return Err(PyErr::fetch(py));
    }
    if unsafe { ffi::PyDict_Update(indexed_dict, old_dict) } != 0 {
        return Err(PyErr::fetch(py));
    }

    let dict_slot = unsafe { soac_module_dict_slot(module)? };
    let old_dict = unsafe { *dict_slot };
    unsafe {
        *dict_slot = indexed_dict;
        ffi::Py_DECREF(old_dict);
    }
    Ok(())
}

unsafe fn soac_new_indexed_module_object(
    module_type: &Bound<'_, PyAny>,
    name: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    unsafe { ffi::PyObject_CallOneArg(module_type.as_ptr(), name) }
}

unsafe fn soac_init_indexed_module_object(
    py: Python<'_>,
    module: *mut ffi::PyObject,
    module_info: ModuleInfo,
) -> PyResult<()> {
    // Py_mod_create can only see (spec, def). Install SOAC's Rust-owned layout
    // metadata immediately after PyModule_FromDefAndSpec returns instead of
    // smuggling it through temporary Python-visible spec attributes.
    let keys_tuple = unsafe { tuple_from_global_names(py, &module_info.indexed_module_keys)? };
    let indexed_dict = unsafe { soac_indexed_module_dict(keys_tuple.as_ptr()) };
    if indexed_dict.is_null() {
        return Err(PyErr::fetch(py));
    }
    if let Err(err) = unsafe { soac_replace_module_dict(py, module, indexed_dict) } {
        unsafe { ffi::Py_DECREF(indexed_dict) };
        return Err(err);
    }

    let info_slot = unsafe { soac_indexed_module_info_slot(module) };
    if unsafe { !(*info_slot).is_null() } {
        return Err(PyRuntimeError::new_err(
            "transformed module ModuleInfo was initialized twice",
        ));
    }
    unsafe { *info_slot = Box::into_raw(Box::new(module_info)) };
    Ok(())
}

unsafe extern "C" fn soac_indexed_module_new(
    subtype: *mut ffi::PyTypeObject,
    args: *mut ffi::PyObject,
    kwargs: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let Some(base_new) = (unsafe { ffi::PyModule_Type.tp_new }) else {
        PyRuntimeError::new_err("module base type does not expose tp_new").restore(py);
        return ptr::null_mut();
    };

    let module = unsafe { base_new(subtype, args, kwargs) };
    if module.is_null() {
        return ptr::null_mut();
    }

    let info_slot = unsafe { soac_indexed_module_info_slot(module) };
    unsafe { ptr::write(info_slot, ptr::null_mut::<ModuleInfo>()) };
    module
}

unsafe extern "C" fn soac_indexed_module_dealloc(module: *mut ffi::PyObject) {
    let module_info = unsafe { *soac_indexed_module_info_slot(module) };
    if !module_info.is_null() {
        unsafe { drop(Box::from_raw(module_info)) };
    }
    let Some(base_dealloc) = (unsafe { ffi::PyModule_Type.tp_dealloc }) else {
        unsafe { ffi::PyObject_Free(module.cast::<c_void>()) };
        return;
    };
    unsafe { base_dealloc(module) };
}

unsafe extern "C" fn soac_ext_module_create(
    spec: *mut ffi::PyObject,
    _def: *mut ffi::PyModuleDef,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let module_type = match soac_indexed_module_type(py) {
        Ok(module_type) => module_type,
        Err(err) => {
            err.restore(py);
            return ptr::null_mut();
        }
    };

    let name = unsafe { ffi::PyObject_GetAttrString(spec, c"name".as_ptr()) };
    if name.is_null() {
        return ptr::null_mut();
    }

    let module = unsafe { soac_new_indexed_module_object(module_type, name) };
    unsafe {
        ffi::Py_DECREF(name);
    }
    module
}

static mut SOAC_EXT_MODULE_SLOTS: [ffi::PyModuleDef_Slot; 2] = [
    ffi::PyModuleDef_Slot {
        slot: ffi::Py_mod_create,
        value: soac_ext_module_create as *mut c_void,
    },
    ffi::PyModuleDef_Slot {
        slot: 0,
        value: ptr::null_mut(),
    },
];

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
    unsafe {
        SOAC_EXT_MODULE_DEF.m_slots = ptr::addr_of_mut!(SOAC_EXT_MODULE_SLOTS).cast();
        ptr::addr_of_mut!(SOAC_EXT_MODULE_DEF)
    }
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

const MODULE_DICT_METADATA_NAMES: &[&str] = &[
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "__builtins__",
    "__file__",
    "__cached__",
    "__path__",
];

static SOAC_INDEXED_MODULE_TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

fn create_soac_indexed_module_type(py: Python<'_>) -> PyResult<Py<PyAny>> {
    unsafe {
        let bases = ffi::PyTuple_Pack(
            1,
            ptr::addr_of_mut!(ffi::PyModule_Type).cast::<ffi::PyObject>(),
        );
        let bases = Bound::from_owned_ptr_or_err(py, bases)?;
        let mut slots = [
            ffi::PyType_Slot {
                slot: ffi::Py_tp_new,
                pfunc: soac_indexed_module_new as *mut c_void,
            },
            ffi::PyType_Slot {
                slot: ffi::Py_tp_dealloc,
                pfunc: soac_indexed_module_dealloc as *mut c_void,
            },
            ffi::PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = ffi::PyType_Spec {
            name: c"_soac_ext.IndexedModuleType".as_ptr(),
            basicsize: (soac_indexed_module_info_offset() + std::mem::size_of::<*mut ModuleInfo>())
                as c_int,
            itemsize: 0,
            flags: (ffi::Py_TPFLAGS_DEFAULT | ffi::Py_TPFLAGS_BASETYPE) as _,
            slots: slots.as_mut_ptr(),
        };
        Bound::from_owned_ptr_or_err(py, ffi::PyType_FromSpecWithBases(&mut spec, bases.as_ptr()))
            .map(Bound::unbind)
    }
}

fn soac_indexed_module_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    SOAC_INDEXED_MODULE_TYPE
        .get_or_try_init(py, || create_soac_indexed_module_type(py))
        .map(|module_type| module_type.bind(py))
}

pub fn indexed_module_type_for_python(py: Python<'_>) -> PyResult<Py<PyAny>> {
    Ok(soac_indexed_module_type(py)?.clone().unbind())
}

fn ensure_module_dict_metadata_names(global_names: &mut Vec<String>) {
    for name in MODULE_DICT_METADATA_NAMES {
        if !global_names.iter().any(|existing| existing == name) {
            global_names.push((*name).to_string());
        }
    }
}

unsafe fn tuple_from_global_names<'py>(
    py: Python<'py>,
    global_names: &[String],
) -> PyResult<Bound<'py, PyTuple>> {
    let tuple = unsafe { ffi::PyTuple_New(global_names.len() as ffi::Py_ssize_t) };
    let tuple = unsafe { Bound::from_owned_ptr_or_err(py, tuple)? }.cast_into::<PyTuple>()?;
    for (index, name) in global_names.iter().enumerate() {
        let item = unsafe {
            ffi::PyUnicode_FromStringAndSize(
                name.as_ptr().cast::<c_char>(),
                name.len() as ffi::Py_ssize_t,
            )
        };
        if item.is_null() {
            return Err(PyErr::fetch(py));
        }
        if unsafe { ffi::PyTuple_SetItem(tuple.as_ptr(), index as ffi::Py_ssize_t, item) } != 0 {
            return Err(PyErr::fetch(py));
        }
    }
    Ok(tuple)
}

pub struct SoacExtModule;

impl SoacExtModule {
    pub fn new(
        py: Python<'_>,
        spec: &Bound<'_, PyAny>,
        compile_session: &Arc<crate::session::CompileSession>,
        mut lowered_module: BlockPyModule<CodegenModuleShape>,
        mut module_info: ModuleInfo,
        original_code_by_function_id: HashMap<RuntimeFunctionId, Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        ensure_module_dict_metadata_names(&mut lowered_module.global_names);
        module_info.indexed_module_keys = lowered_module.global_names.clone();
        let source_hash = module_info.hash;
        let module_cache_source = module_info.cache_source;
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
        unsafe { soac_init_indexed_module_object(py, module.as_ptr(), module_info)? };
        if unsafe { ffi::PyModule_ExecDef(module.as_ptr(), soac_ext_module_def()) } != 0 {
            return Err(PyErr::fetch(py));
        }
        let state = soac_ext_module_state(&module)?;
        unsafe {
            (*state).init(
                py,
                compile_session,
                lowered_module,
                original_code_by_function_id,
                module_name,
                package_name,
                source_hash,
                module_cache_source,
            )?
        };
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
}

#[cfg(test)]
mod test {
    use super::*;
    use pyo3::types::PyModule;
    use soac_core::profile::COUNTER_DUMP_MAGIC;
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use soac_lowering::passes::instrument_bb_module_with_block_entry_counters;
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
            inline_plan: plan_inline_candidates(&lowered),
            source_hash: 0,
            module_cache_source: None,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0)].into_boxed_slice(),
            counter_values: vec![3].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: String::new(),
            original_code_by_function_id: HashMap::new(),
            precompiled_module_runtime: OnceLock::new(),
            jit_module_plan: OnceLock::new(),
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
    fn counter_dump_record_includes_deopt_entry_source_and_reason() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .expect("missing lowered function f");
        let function_id = function.function_id;
        let entry_label = function.entry_block().label;
        let entry_label_text = entry_label.to_string();
        lowered.counter_defs.push(CounterDef {
            id: CounterId(0),
            scope: CounterScope::This,
            kind: "deopt_entry_guard_miss".to_string(),
            site: CounterSite::DeoptEntry {
                function_id,
                source: DeoptEntrySource::BeforeTerm {
                    block_label: entry_label,
                },
            },
        });

        let shared_state = SharedModuleState {
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            inline_plan: plan_inline_candidates(&lowered),
            source_hash: 0,
            module_cache_source: None,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0)].into_boxed_slice(),
            counter_values: vec![5].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: String::new(),
            original_code_by_function_id: HashMap::new(),
            precompiled_module_runtime: OnceLock::new(),
            jit_module_plan: OnceLock::new(),
        };

        let record = shared_state
            .counter_dump_record()
            .expect("counter dump record should be present");
        let row = record
            .rows
            .iter()
            .find(|row| row.kind == "deopt_entry_guard_miss")
            .expect("deopt-entry counter row should be present");
        assert_eq!(row.scope, "this");
        assert_eq!(row.site_kind, "deopt_entry");
        assert_eq!(row.function_id, Some(function_id));
        assert_eq!(row.current_function_id, Some(function_id));
        assert_eq!(row.function_qualname.as_deref(), Some("f"));
        assert_eq!(row.block_label.as_deref(), Some(entry_label_text.as_str()));
        assert_eq!(row.instr_id, None);
        assert_eq!(row.value, 5);
    }

    #[test]
    fn counter_scope_controls_storage_sharing() {
        let counter_defs = vec![
            CounterDef {
                id: CounterId(0),
                scope: CounterScope::Function,
                kind: "runtime_incref".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(RuntimeFunctionId::from_raw_parts(0, 7)),
                    instr_id: None,
                },
            },
            CounterDef {
                id: CounterId(1),
                scope: CounterScope::Function,
                kind: "runtime_incref".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(RuntimeFunctionId::from_raw_parts(0, 7)),
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
                    function_id: RuntimeFunctionId::from_raw_parts(0, 7),
                    block_label: soac_core::block_py::BlockLabel::from_index(0),
                },
            },
        ];

        let (slots_by_id, counter_values, top_value_counters) =
            build_counter_storage(&counter_defs).expect("counter storage should build");
        assert_eq!(counter_values.len(), 3);
        assert!(top_value_counters.is_empty());
        assert_eq!(slots_by_id[0], slots_by_id[1]);
        assert_eq!(slots_by_id[2], slots_by_id[3]);
        assert_ne!(slots_by_id[0], slots_by_id[2]);
        assert_ne!(slots_by_id[0], slots_by_id[4]);
        assert_ne!(slots_by_id[2], slots_by_id[4]);
    }

    #[test]
    fn type_key_layout_events_use_profile_type_ids() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"
class Point:
    pass

events = [(Point, 'x', 0), (Point, 'y', 1)]
",
                c"type_events.py",
                c"type_event_test",
            )
            .expect("test module should execute");
            let events = module.getattr("events").expect("events should exist");
            let (layouts, type_table) = snapshot_type_key_layout_events_bound(events.as_any())
                .expect("type key layout events should snapshot");

            assert_eq!(layouts.len(), 2);
            let owner_type_id = layouts[0].owner_type_id;
            assert_ne!(owner_type_id, 0);
            assert!(
                layouts
                    .iter()
                    .all(|layout| layout.owner_type_id == owner_type_id),
                "all events for the same owner type should use one profile type id"
            );
            assert_eq!(layouts[0].key, "x");
            assert_eq!(layouts[0].index, 0);
            assert_eq!(layouts[1].key, "y");
            assert_eq!(layouts[1].index, 1);

            assert_eq!(type_table.len(), 1);
            assert_eq!(type_table[0].type_id, owner_type_id);
            assert_eq!(type_table[0].key.module_name, "type_event_test");
            assert_eq!(type_table[0].key.qualname, "Point");
        });
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
            inline_plan: plan_inline_candidates(&lowered),
            source_hash: 0,
            module_cache_source: None,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0), CounterRuntimeSlot::Scalar(1)]
                .into_boxed_slice(),
            counter_values: vec![5, 8].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: "pkg".to_string(),
            original_code_by_function_id: HashMap::new(),
            precompiled_module_runtime: OnceLock::new(),
            jit_module_plan: OnceLock::new(),
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
