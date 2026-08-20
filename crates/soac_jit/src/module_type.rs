use crate::counter::{CounterEntry, GilTopValueCounter, TopValueCounter};
use crate::jit::JitCodegenStats;
use crate::module_constants::{ModuleCodegenConstants, load_runtime_name_owned_by_id};
use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyList, PyModule, PyTuple};
use soac_config::SoacEnvConfig;
use soac_config::SpecializationMode;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, CounterDef, CounterId, CounterScope, CounterSite,
    DeoptEntrySource, FunctionExecutionMode, FunctionKind, RuntimeFunctionId, RuntimeName,
    current_instr_locations,
};
use soac_core::profile::{
    CounterDumpBranchValue, CounterDumpKeyLayout, CounterDumpRecord, CounterDumpRow,
    CounterDumpTypeKey, CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry,
};
use soac_instrument::InstrumentationConfig;
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::plan_v3::LateBoundOwnerFieldSpecializationPlan;
use soac_opt::pipeline_v3::late_bound_owner_field_site_catalog;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{c_char, c_int, c_void};
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    pub strict_runtime: Option<&'a crate::StrictModuleRuntimeState>,
}

/// Complete only objects produced by this authenticated module execution.
/// Class-owned methods are adopted by their actual protected class, never by
/// source membership alone: a framework class that declined participation
/// must not have its methods frozen through this independent module path.
pub fn finalize_strict_module_contents(
    py: Python<'_>,
    state: &crate::StrictModuleRuntimeState,
    shared: &SharedModuleState,
) -> PyResult<()> {
    use crate::strict_function::{
        authenticate_strict_function, eligible_function, finalize_eligible_function,
    };
    use crate::strict_module::StrictPendingKind;
    use soac_core::block_py::CallableSourceRole;

    let verified = shared.verified_strict_module().ok_or_else(|| {
        crate::strict_runtime_unavailable(py, "strict finalization has no verified module")
    })?;
    // Upgrade one weak target at a time. Sealing an earlier target must not
    // extend the lifetime of unrelated functions or their captured objects.
    // A GC callback during optional publication can construct another target;
    // repeat both phases until their existing registries are quiescent.
    loop {
        let mut progressed = false;
        while let Some((kind, object)) = state.next_pending(py)? {
            progressed = true;
            let object = object.into_bound(py);
            match kind {
                StrictPendingKind::Function { function_id } => {
                    // Determine adoption responsibility from the private pending
                    // registry and immutable source policy before authenticating
                    // mutable native metadata. Dynamic functions and functions
                    // owned by an unparticipating class may have replaced code;
                    // merely remaining alive must not give them a frozen contract.
                    let template = shared
                        .lookup_function_template(function_id)
                        .map_err(|error| crate::strict_runtime_unavailable(py, error))?
                        .ok_or_else(|| {
                            crate::strict_runtime_unavailable(
                                py,
                                "pending function has no template",
                            )
                        })?;
                    if let Some(origin) = template.function().scope.source_origin.as_ref() {
                        match origin.role {
                            CallableSourceRole::TypeParameterScope => continue,
                            CallableSourceRole::SourceFunction => {
                                let class_owned = verified
                                    .type_facts()
                                    .facts()
                                    .source_class_owner(&origin.definition)
                                    .is_some();
                                if !eligible_function(shared, Some(origin))
                                    || (class_owned
                                        && !crate::strict_function::function_awaits_module_nominals(
                                            py, &object,
                                        )?)
                                {
                                    continue;
                                }
                                // Only an already mandatorily frozen method's
                                // module-only stage reaches full authentication
                                // below. Source membership never adopts a
                                // dynamic/unparticipating framework method.
                            }
                            CallableSourceRole::AnnotationProvider
                                if matches!(
                                    origin.definition.definition_kind,
                                    soac_contracts::DefinitionKind::Function
                                        | soac_contracts::DefinitionKind::Class
                                        | soac_contracts::DefinitionKind::TypeAlias
                                        | soac_contracts::DefinitionKind::Parameter
                                ) =>
                            {
                                // Functions/classes own dictionary-provider
                                // adoption. Type evaluators deliberately remain
                                // provenance-only, with no delayed target lookup.
                                continue;
                            }
                            _ => {}
                        }
                    }
                    let Some(auth) = authenticate_strict_function(py, &object)? else {
                        return Err(crate::strict_runtime_unavailable(
                            py,
                            "pending strict function lost its owner",
                        ));
                    };
                    if !std::ptr::eq(auth.module_state()?.as_ref(), shared)
                        || auth.function_id()? != function_id
                    {
                        return Err(crate::strict_runtime_unavailable(
                            py,
                            "pending function belongs to another execution",
                        ));
                    }
                    if let Some(origin) = auth.origin() {
                        if finalize_eligible_function(py, &object, &origin.definition)?
                            && !auth.capability_nominal_bindings().is_empty()
                        {
                            state.defer_capability_publication(
                                py,
                                StrictPendingKind::Function { function_id },
                                &object,
                            )?;
                        }
                    }
                }
                StrictPendingKind::InterpreterFunction { .. } => {
                    return Err(crate::strict_runtime_unavailable(
                        py,
                        "interpreter definition reached compiled-module finalization",
                    ));
                }
                StrictPendingKind::Class { source } => {
                    if !crate::strict_class::finalize_class(py, &object, &source)? {
                        return Err(crate::strict_runtime_unavailable(
                            py,
                            "pending strict class lost its installed contract",
                        ));
                    }
                    state.defer_capability_publication(
                        py,
                        StrictPendingKind::Class { source },
                        &object,
                    )?;
                }
            }
        }
        // Every live target is already permanently sealed. This second weak drain
        // only fills missing optimization slots from exact captured nominal types;
        // source order cannot turn a forward annotation into a permanent miss.
        while let Some((kind, object)) = state.next_capability_publication(py)? {
            progressed = true;
            let object = object.into_bound(py);
            match kind {
                StrictPendingKind::Function { function_id } => {
                    let auth = authenticate_strict_function(py, &object)?.ok_or_else(|| {
                        crate::strict_runtime_unavailable(py, "capability function lost its owner")
                    })?;
                    if !std::ptr::eq(auth.module_state()?.as_ref(), shared)
                        || auth.function_id()? != function_id
                        || !auth.is_finalized()
                    {
                        return Err(crate::strict_runtime_unavailable(
                            py,
                            "capability function changed its sealed execution identity",
                        ));
                    }
                    crate::strict_optimization::bind_nominal_function_capabilities(py, &object)?;
                }
                StrictPendingKind::InterpreterFunction { .. } => {
                    return Err(crate::strict_runtime_unavailable(
                        py,
                        "interpreter definition cannot publish compiled capabilities",
                    ));
                }
                StrictPendingKind::Class { source } => {
                    if !crate::strict_class::finalize_class(py, &object, &source)? {
                        return Err(crate::strict_runtime_unavailable(
                            py,
                            "capability class lost its installed contract",
                        ));
                    }
                }
            }
        }
        if !progressed {
            break;
        }
    }
    Ok(())
}

#[repr(C)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleInfo {
    pub hash: u64,
    pub indexed_module_keys: Vec<String>,
}

pub struct SharedModuleState {
    pub(crate) strict_module: Option<Arc<crate::VerifiedStrictModule>>,
    pub(crate) strict_execution: Option<crate::strict_module::StrictModuleExecutionRef>,
    pub(crate) late_bound_owner_fields: LateBoundOwnerFieldRuntime,
    pub lowered_module: BlockPyModule<BlockPyModuleShape>,
    pub module_name: String,
    pub package_name: String,
    pub source_hash: u64,
    pub codegen_constants: ModuleCodegenConstants,
    storage_instance_key: usize,
    function_index_by_id: HashMap<RuntimeFunctionId, usize>,
    function_templates:
        Mutex<HashMap<RuntimeFunctionId, Arc<crate::FunctionInstantiationTemplate>>>,
    pub(crate) original_code_by_function_id: crate::strict_admission::OriginalCodeStorage,
    module_constant_objs: Vec<Py<PyAny>>,
    // Each non-null slot owns one runtime-name reference for this module state; lookups return a
    // fresh owned reference by INCREFing the cached pointer.
    runtime_name_cache: Box<[AtomicUsize]>,
    counter_slots_by_id: Box<[CounterRuntimeSlot]>,
    counter_values: Box<[u64]>,
    top_value_counters: Box<[GilTopValueCounter]>,
    deopt_entry_counters: Mutex<DeoptEntryCounterRegistry>,
    counter_dump_flush_tracker: Mutex<CounterDumpFlushTracker>,
}

/// Counters for one finalized deopt table. These never resize or alias the
/// module's already-published scalar storage. The owning module retains only
/// this Python-free diagnostic state, not the table or its captured objects.
pub(crate) struct DeoptEntryCounters {
    entries: Box<[(CounterDef, AtomicU64)]>,
}

impl DeoptEntryCounters {
    pub(crate) fn record(&self, ordinal: usize) {
        self.entries[ordinal].1.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> impl Iterator<Item = (&CounterDef, u64)> {
        self.entries
            .iter()
            .map(|(definition, value)| (definition, value.load(Ordering::Relaxed)))
    }
}

#[derive(Default)]
struct DeoptEntryCounterRegistry {
    sets: Vec<Arc<DeoptEntryCounters>>,
    count: usize,
}

#[repr(C)]
pub(crate) struct LateBoundOwnerFieldCell {
    pub(crate) owner_weakref: AtomicUsize,
    pub(crate) type_version: AtomicUsize,
    pub(crate) slot_offset: AtomicUsize,
}

pub(crate) struct LateBoundOwnerFieldRuntime {
    pub(crate) sites: Box<[(RuntimeFunctionId, LateBoundOwnerFieldSpecializationPlan)]>,
    pub(crate) cells: Box<[LateBoundOwnerFieldCell]>,
    pub(crate) owner_weakrefs: Mutex<Vec<Py<PyAny>>>,
}

impl LateBoundOwnerFieldRuntime {
    fn for_module(module: &BlockPyModule<BlockPyModuleShape>, module_name: &str) -> Self {
        let sites = late_bound_owner_field_site_catalog(module, module_name).into_boxed_slice();
        let cells = sites
            .iter()
            .map(|_| LateBoundOwnerFieldCell {
                owner_weakref: AtomicUsize::new(0),
                type_version: AtomicUsize::new(0),
                slot_offset: AtomicUsize::new(0),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            sites,
            cells,
            owner_weakrefs: Mutex::new(Vec::new()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CounterDumpFlushStatus {
    InProgress,
    Complete,
}

#[derive(Default)]
struct CounterDumpFlushTracker {
    paths: HashMap<PathBuf, CounterDumpFlushStatus>,
    module_cleared: bool,
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
    Branches { start: usize, len: usize },
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
    pub(crate) fn register_deopt_entry_counters(
        &self,
        function_id: RuntimeFunctionId,
        sources: Vec<DeoptEntrySource>,
    ) -> Result<Arc<DeoptEntryCounters>, String> {
        if self.lookup_function(function_id).is_none() {
            return Err(format!(
                "deopt counters have unknown function {function_id}"
            ));
        }
        let mut registry = self
            .deopt_entry_counters
            .lock()
            .map_err(|_| "deopt counter registry is poisoned".to_string())?;
        let first_id = self
            .lowered_module
            .counter_defs
            .len()
            .checked_add(registry.count)
            .ok_or("deopt counter id overflow")?;
        let end_id = first_id
            .checked_add(sources.len())
            .ok_or("deopt counter id overflow")?;
        u32::try_from(end_id).map_err(|_| "deopt counter ids do not fit the dump format")?;
        let entries = sources
            .into_iter()
            .enumerate()
            .map(|(ordinal, source)| {
                (
                    CounterDef::scalar(
                        CounterId(first_id + ordinal),
                        CounterScope::This,
                        "deopt_entry_guard_miss",
                        CounterSite::DeoptEntry {
                            function_id,
                            source,
                        },
                    ),
                    AtomicU64::new(0),
                )
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let counters = Arc::new(DeoptEntryCounters { entries });
        registry.count += counters.entries.len();
        registry.sets.push(Arc::clone(&counters));
        Ok(counters)
    }

    fn deopt_entry_counter_sets(&self) -> Vec<Arc<DeoptEntryCounters>> {
        self.deopt_entry_counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sets
            .clone()
    }

    pub fn verified_strict_module(&self) -> Option<&crate::VerifiedStrictModule> {
        self.strict_module.as_deref()
    }

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
    ) -> Option<&BlockPyFunction<BlockPyModuleShape>> {
        let function_index = self.function_index_by_id.get(&function_id).copied()?;
        let function = self.lowered_module.callable_defs.get(function_index)?;
        assert_eq!(function.function_id, function_id);
        Some(function)
    }

    pub(crate) fn lookup_function_template(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<Arc<crate::FunctionInstantiationTemplate>>, String> {
        if function_id == RuntimeFunctionId::global() {
            return Ok(None);
        }
        if let Some(template) = self
            .function_templates
            .lock()
            .map_err(|_| "function template cache lock poisoned".to_string())?
            .get(&function_id)
            .cloned()
        {
            return Ok(Some(template));
        }
        let Some(function) = self.lookup_function(function_id) else {
            return Ok(None);
        };
        let template = Arc::new(crate::FunctionInstantiationTemplate::from_function(
            function,
        )?);
        let mut templates = self
            .function_templates
            .lock()
            .map_err(|_| "function template cache lock poisoned".to_string())?;
        Ok(Some(
            templates
                .entry(function_id)
                .or_insert_with(|| template)
                .clone(),
        ))
    }

    fn deopt_entry_source_block_label(
        &self,
        function_id: RuntimeFunctionId,
        source: DeoptEntrySource,
    ) -> String {
        match source {
            DeoptEntrySource::BlockEntry { block_label }
            | DeoptEntrySource::BeforeTerm { block_label } => block_label.to_string(),
            DeoptEntrySource::BeforeInstr { instr_id } => self
                .lookup_function(function_id)
                .and_then(|function| {
                    current_instr_locations(function)
                        .get(&instr_id)
                        .map(|location| location.block_label().to_string())
                })
                .unwrap_or_default(),
        }
    }

    pub(crate) fn lookup_direct_call_target_function(
        &self,
        compile_session: &crate::session::CompileSession,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<BlockPyFunction<BlockPyModuleShape>>, String> {
        if function_id == RuntimeFunctionId::global() {
            return Ok(None);
        }
        if let Some(function) = self.lookup_function(function_id) {
            return Ok(self.admits_function(function).then(|| function.clone()));
        }
        if function_id.runtime_module_id().as_u32() == self.module_id() {
            return Ok(None);
        }
        Ok(compile_session
            .lookup_shared_function(function_id)?
            .filter(|(shared_state, function)| shared_state.admits_function(function))
            .map(|(_shared_state, function)| function))
    }

    pub fn lookup_original_code(&self, function_id: RuntimeFunctionId) -> Option<&Py<PyAny>> {
        self.original_code_by_function_id.get(&function_id)
    }

    pub(crate) fn lookup_generator_expression_code(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<&Py<PyAny>> {
        self.original_code_by_function_id
            .generator_expression_code(&function_id)
    }

    pub(crate) fn admits_function<S: soac_core::block_py::ModuleShape>(
        &self,
        function: &BlockPyFunction<S>,
    ) -> bool {
        match (
            self.verified_strict_module(),
            self.original_code_by_function_id.authenticated(),
        ) {
            (Some(verified), Some(catalog)) => catalog.admits(verified, function),
            _ => false,
        }
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
            CounterRuntimeSlot::Branches { start, len } => self
                .counter_values
                .get(start..start.saturating_add(len))
                .unwrap_or_default()
                .iter()
                .copied()
                .sum(),
            CounterRuntimeSlot::TopValues(_) => 0,
        }
    }

    pub fn counter_branch_value(
        &self,
        counter_id: CounterId,
        branch_id: soac_core::block_py::CounterBranchId,
    ) -> u64 {
        let Some(slot) = self.counter_slots_by_id.get(counter_id.0).copied() else {
            return 0;
        };
        match slot {
            CounterRuntimeSlot::Branches { start, len } if branch_id.0 < len => self
                .counter_values
                .get(start + branch_id.0)
                .copied()
                .unwrap_or_default(),
            CounterRuntimeSlot::Scalar(_)
            | CounterRuntimeSlot::Branches { .. }
            | CounterRuntimeSlot::TopValues(_) => 0,
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
        if !self.admits_function(&function) {
            return Err(
                "direct compilation requires an individually authenticated strict template".into(),
            );
        }
        if function.execution_mode() == FunctionExecutionMode::Interpreted {
            return Ok(None);
        }
        if let Some(handle) = compile_session
            .process_jit()?
            .lookup_ready_direct_function(&function)?
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
        function: &BlockPyFunction<BlockPyModuleShape>,
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
        if !InstrumentationConfig::from_env_config(&env_config)
            .specialization_runtime_logging_enabled()
        {
            return;
        }
        let deopt_counters = self.deopt_entry_counter_sets();
        for (counter, scalar_value) in self
            .lowered_module
            .counter_defs
            .iter()
            .map(|counter| (counter, self.counter_value(counter.id)))
            .chain(
                deopt_counters
                    .iter()
                    .flat_map(|counters| counters.snapshot()),
            )
        {
            let kind = counter.kind.as_str();
            let is_specialization_counter = matches!(
                kind,
                "global_indexed"
                    | "field_access"
                    | "getitem_specialized"
                    | "setitem_specialized"
                    | "call_direct"
                    | "deopt_entry_guard_miss"
            );
            if !is_specialization_counter {
                continue;
            }
            let branch_values = counter
                .branches
                .iter()
                .enumerate()
                .filter_map(|(branch_index, branch)| {
                    let value = self.counter_branch_value(
                        counter.id,
                        soac_core::block_py::CounterBranchId(branch_index),
                    );
                    (value > 0).then_some((branch.name.as_str(), value))
                })
                .collect::<Vec<_>>();
            if scalar_value == 0 && branch_values.is_empty() {
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
                    self.deopt_entry_source_block_label(*function_id, *source),
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
            if branch_values.is_empty() {
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
                    value = scalar_value,
                    "specialization_runtime",
                );
            } else {
                for (branch, value) in branch_values {
                    info!(
                        target: "soac_specialization_runtime",
                        event = "soac.specialization_runtime",
                        module_name = self.module_name,
                        package_name = self.package_name,
                        kind,
                        branch,
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
        }
    }

    pub fn counter_dump_record(&self) -> Option<CounterDumpRecord> {
        let module_keys = self.counter_dump_module_keys();
        let (type_keys, type_table) = self.counter_dump_type_key_layouts();

        let mut rows = Vec::new();
        let deopt_counters = self.deopt_entry_counter_sets();
        for (counter, scalar_value) in self
            .lowered_module
            .counter_defs
            .iter()
            .map(|counter| (counter, self.counter_value(counter.id)))
            .chain(
                deopt_counters
                    .iter()
                    .flat_map(|counters| counters.snapshot()),
            )
        {
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
                    Some(self.deopt_entry_source_block_label(*function_id, *source)),
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
                branch_values: Vec::new(),
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
                if counter.is_branch_counter() {
                    row.branch_values = counter
                        .branches
                        .iter()
                        .enumerate()
                        .map(|(branch_index, branch)| CounterDumpBranchValue {
                            branch: branch.name.clone(),
                            value: self.counter_branch_value(
                                counter.id,
                                soac_core::block_py::CounterBranchId(branch_index),
                            ),
                        })
                        .collect();
                    row.value = row.branch_values.iter().map(|branch| branch.value).sum();
                } else {
                    row.value = scalar_value;
                }
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
        snapshot_type_key_layout_events(self.module_name.as_str())
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

    pub(crate) fn flush_counter_dump_file_once(&self, path: &Path) -> Result<(), String> {
        {
            let mut tracker = self
                .counter_dump_flush_tracker
                .lock()
                .map_err(|_| "counter dump flush tracker lock poisoned".to_string())?;
            if tracker.module_cleared || tracker.paths.contains_key(path) {
                return Ok(());
            }
            tracker
                .paths
                .insert(path.to_path_buf(), CounterDumpFlushStatus::InProgress);
        }

        // Capturing type layouts can invoke Python attribute callbacks, so neither the
        // per-module tracker nor the session registry may remain locked during serialization.
        let result = self.append_counter_dump_file(path);
        let mut tracker = self
            .counter_dump_flush_tracker
            .lock()
            .map_err(|_| "counter dump flush tracker lock poisoned".to_string())?;
        if result.is_ok() {
            tracker
                .paths
                .insert(path.to_path_buf(), CounterDumpFlushStatus::Complete);
        } else {
            tracker.paths.remove(path);
        }
        result
    }

    fn mark_counter_dump_module_cleared(&self) -> Result<(), String> {
        let mut tracker = self
            .counter_dump_flush_tracker
            .lock()
            .map_err(|_| "counter dump flush tracker lock poisoned".to_string())?;
        tracker.module_cleared = true;
        Ok(())
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
            | "call_direct_targets"
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
    let mut scalar_slot_count = 0usize;
    for counter in counter_defs {
        if counter.id.0 >= slots_by_id.len() {
            return Err(format!(
                "counter id {} is out of range for {} counter defs",
                counter.id.0,
                counter_defs.len()
            ));
        }
        let key = counter_storage_key(counter)?;
        if counter.is_branch_counter() && counter_uses_call_target_storage(counter) {
            return Err(format!(
                "counter {} ({}) cannot use both branch and top-value storage",
                counter.id.0, counter.kind
            ));
        }
        let slot = if counter_uses_call_target_storage(counter) {
            let slot = if let Some(slot) = top_values_slot_by_key.get(&key).copied() {
                slot
            } else {
                let slot = top_values_slot_by_key.len();
                top_values_slot_by_key.insert(key, slot);
                slot
            };
            CounterRuntimeSlot::TopValues(slot)
        } else if counter.is_branch_counter() {
            let start = if let Some(start) = scalar_slot_by_key.get(&key).copied() {
                start
            } else {
                let start = scalar_slot_count;
                scalar_slot_by_key.insert(key, start);
                scalar_slot_count += counter.branches.len();
                start
            };
            CounterRuntimeSlot::Branches {
                start,
                len: counter.branches.len(),
            }
        } else {
            let slot = if let Some(slot) = scalar_slot_by_key.get(&key).copied() {
                slot
            } else {
                let slot = scalar_slot_count;
                scalar_slot_by_key.insert(key, slot);
                scalar_slot_count += 1;
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
        scalar_slot_count,
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
) -> PyResult<Vec<Py<PyAny>>> {
    if module_name == "soac.runtime" {
        codegen_constants.build_python_constants_for_soac_runtime(py)
    } else {
        codegen_constants.build_python_constants(py)
    }
}

type OriginalCodeByQualname = HashMap<String, VecDeque<Py<PyAny>>>;

fn collect_original_code_objects(
    code: &Bound<'_, PyAny>,
    code_type: &Bound<'_, PyAny>,
    by_qualname: &mut OriginalCodeByQualname,
) -> PyResult<()> {
    let qualname = code.getattr("co_qualname")?.extract::<String>()?;
    by_qualname
        .entry(qualname)
        .or_default()
        .push_back(code.clone().unbind());

    let consts = code.getattr("co_consts")?;
    let const_count = unsafe { ffi::PyTuple_Size(consts.as_ptr()) };
    if const_count < 0 {
        return Err(PyErr::fetch(code.py()));
    }
    for index in 0..const_count {
        let item = unsafe { ffi::PyTuple_GetItem(consts.as_ptr(), index) };
        if item.is_null() {
            return Err(PyErr::fetch(code.py()));
        }
        let item = unsafe { Bound::from_borrowed_ptr(code.py(), item) };
        if item.is_instance(code_type)? {
            collect_original_code_objects(&item, code_type, by_qualname)?;
        }
    }
    Ok(())
}

fn original_code_lookup_key(function: &BlockPyFunction<BlockPyModuleShape>) -> Option<&str> {
    if function.execution_mode() == FunctionExecutionMode::Interpreted {
        return None;
    }
    let qualname = function.names.qualname.as_str();
    if qualname == "_dp_module_init"
        || function.names.fn_name == "_dp_resume"
        || function.names.bind_name.starts_with("_dp_class_ns_")
        || function.names.bind_name.starts_with("_dp_define_class_")
    {
        return None;
    }
    Some(qualname)
}

pub fn match_original_code_to_functions(
    py: Python<'_>,
    module_code: &Bound<'_, PyAny>,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
) -> PyResult<HashMap<RuntimeFunctionId, Py<PyAny>>> {
    let code_type = PyModule::import(py, "types")?.getattr("CodeType")?;
    let mut code_by_qualname = HashMap::new();
    collect_original_code_objects(module_code, &code_type, &mut code_by_qualname)?;

    let mut code_by_function_id = HashMap::new();
    for function in &lowered_module.callable_defs {
        let Some(qualname) = original_code_lookup_key(function) else {
            continue;
        };
        let Some(codes) = code_by_qualname.get_mut(qualname) else {
            continue;
        };
        let Some(code) = codes.pop_front() else {
            continue;
        };
        code_by_function_id.insert(function.function_id, code);
    }
    Ok(code_by_function_id)
}

/// Compile only authenticated source bytes through the actual native compiler.
/// Both execution backends consume this tuple directly; a Python-supplied
/// tuple, code stamp, or public future bit cannot enter this constructor.
pub(crate) fn compile_verified_native_details<'py>(
    py: Python<'py>,
    verified: &crate::VerifiedStrictModule,
) -> PyResult<Bound<'py, PyTuple>> {
    unsafe extern "C" {
        fn PySoac_CompileVerifiedSourceDetails(
            source: *const c_char,
            length: ffi::Py_ssize_t,
            filename: *mut ffi::PyObject,
            optimize: c_int,
        ) -> *mut ffi::PyObject;
        fn PyCode_GetSoacStrictSourceId(code: *mut ffi::PyObject) -> u64;
    }
    let interpreter_id = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
    if interpreter_id != verified.interpreter_id() {
        return Err(crate::strict_runtime_unavailable(
            py,
            "strict native source interpreter mismatch",
        ));
    }
    let source = std::str::from_utf8(verified.source())
        .map_err(|_| crate::strict_runtime_unavailable(py, "strict source bytes are not UTF-8"))?;
    let path = verified
        .source_path()
        .to_str()
        .ok_or_else(|| crate::strict_runtime_unavailable(py, "strict source path is not UTF-8"))?;
    let filename = pyo3::types::PyString::new(py, path);
    let details = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            PySoac_CompileVerifiedSourceDetails(
                source.as_ptr().cast(),
                source.len() as ffi::Py_ssize_t,
                filename.as_ptr(),
                -1,
            ),
        )?
    }
    .cast_into::<PyTuple>()?;
    if !details.is_exact_instance_of::<PyTuple>() || details.len() != 3 {
        return Err(crate::strict_runtime_unavailable(
            py,
            "invalid native source details",
        ));
    }
    let root = details.get_item(0)?;
    if unsafe { ffi::Py_TYPE(root.as_ptr()) } != ptr::addr_of_mut!(ffi::PyCode_Type)
        || unsafe { PyCode_GetSoacStrictSourceId(root.as_ptr()) } == 0
    {
        return Err(crate::strict_runtime_unavailable(
            py,
            "native compiler did not authenticate strict source",
        ));
    }
    Ok(details)
}

/// The one privately compiled native root held across lowering. Ordinary
/// source-string tuples never grant authority: only this constructor calls the
/// native compiler on the verified bytes, and the same owned root is consumed
/// into the admission catalog. This temporary owner is not stored in an Arc or
/// added as a second hidden Python edge in SharedModuleState.
pub struct CompiledStrictSource {
    native_root: Py<PyAny>,
    source: soac_core::block_py::StrictModuleSource,
    interpreter_id: i64,
    canonical_annotations: Arc<soac_lowering::CanonicalAnnotationStrings>,
    canonical_class_bindings: Arc<soac_lowering::CanonicalClassBindings>,
}

impl CompiledStrictSource {
    pub fn compile(py: Python<'_>, verified: &crate::VerifiedStrictModule) -> PyResult<Self> {
        let details = compile_verified_native_details(py, verified)?;
        let interpreter_id = verified.interpreter_id();
        let source = std::str::from_utf8(verified.source()).map_err(|_| {
            crate::strict_runtime_unavailable(py, "strict source bytes are not UTF-8")
        })?;
        let native_root = details.get_item(0)?;
        let line_starts = std::iter::once(0)
            .chain(
                source
                    .bytes()
                    .enumerate()
                    .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
            )
            .collect::<Vec<_>>();
        let offset = |line: usize, column: usize| -> PyResult<u32> {
            let index = line.checked_sub(1).ok_or_else(|| {
                crate::strict_runtime_unavailable(py, "native annotation line is not one-based")
            })?;
            let start = *line_starts.get(index).ok_or_else(|| {
                crate::strict_runtime_unavailable(py, "native annotation line is outside source")
            })?;
            let end = line_starts.get(index + 1).copied().unwrap_or(source.len());
            if column > end - start {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "native annotation column is outside source line",
                ));
            }
            u32::try_from(start + column).map_err(|_| {
                crate::strict_runtime_unavailable(
                    py,
                    "native annotation range exceeds source coordinates",
                )
            })
        };
        let rows = details.get_item(1)?.cast_into::<PyTuple>()?;
        if !rows.is_exact_instance_of::<PyTuple>() {
            return Err(crate::strict_runtime_unavailable(
                py,
                "native annotation strings are not immutable",
            ));
        }
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows.iter() {
            if !row.is_exact_instance_of::<PyTuple>() {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "invalid native annotation string row",
                ));
            }
            let (line, column, end_line, end_column, text) =
                row.extract::<(usize, usize, usize, usize, String)>()?;
            entries.push((
                soac_contracts::SourceRange::new(
                    offset(line, column)?,
                    offset(end_line, end_column)?,
                ),
                text,
            ));
        }
        let canonical_annotations = Arc::new(
            soac_lowering::CanonicalAnnotationStrings::from_native_entries(source, entries)
                .map_err(|error| crate::strict_runtime_unavailable(py, error.to_string()))?,
        );
        let canonical_class_bindings = Arc::new(crate::class_bindings::decode(
            py,
            source,
            &native_root,
            details.get_item(2)?,
        )?);
        Ok(Self {
            native_root: native_root.unbind(),
            source: soac_core::block_py::StrictModuleSource::from_verified(verified.type_facts()),
            interpreter_id,
            canonical_annotations,
            canonical_class_bindings,
        })
    }

    pub fn canonical_annotations(&self) -> Arc<soac_lowering::CanonicalAnnotationStrings> {
        Arc::clone(&self.canonical_annotations)
    }

    pub fn canonical_class_bindings(&self) -> Arc<soac_lowering::CanonicalClassBindings> {
        Arc::clone(&self.canonical_class_bindings)
    }

    pub fn into_function_catalog(
        self,
        py: Python<'_>,
        verified: &crate::VerifiedStrictModule,
        lowered_module: &BlockPyModule<BlockPyModuleShape>,
    ) -> PyResult<crate::AuthenticatedCodeCatalog> {
        let thread = unsafe { ffi::PyThreadState_Get() };
        let interpreter = unsafe { ffi::PyThreadState_GetInterpreter(thread) };
        let interpreter_id = unsafe { ffi::PyInterpreterState_GetID(interpreter) };
        if interpreter_id != self.interpreter_id
            || interpreter_id != verified.interpreter_id()
            || !self.source.matches_verified(verified.type_facts())
            || !lowered_module
                .strict_source
                .as_ref()
                .is_some_and(|source| source == &self.source)
        {
            return Err(crate::strict_runtime_unavailable(
                py,
                "strict native source owner mismatch",
            ));
        }
        match_compiled_strict_functions(
            py,
            verified,
            lowered_module,
            self.native_root.into_bound(py),
            &self.canonical_class_bindings,
        )
    }
}

fn match_compiled_strict_functions(
    py: Python<'_>,
    verified: &crate::VerifiedStrictModule,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
    native_root: Bound<'_, PyAny>,
    class_bindings: &soac_lowering::CanonicalClassBindings,
) -> PyResult<crate::AuthenticatedCodeCatalog> {
    let tree = crate::class_bindings::code_tree(py, &native_root)?;
    if tree.len() != class_bindings.nodes().len() {
        return Err(crate::strict_runtime_unavailable(
            py,
            "native class metadata lost its owned root",
        ));
    }
    let (native_codes, native_parents): (Vec<_>, Vec<_>) = tree
        .into_iter()
        .map(|node| (node.code, node.parent.map(|id| id.0 as usize)))
        .unzip();
    let line_starts = std::iter::once(0)
        .chain(
            verified
                .source()
                .iter()
                .enumerate()
                .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1)),
        )
        .collect::<Vec<_>>();
    let mut matched = HashSet::new();
    let mut source_nodes = HashMap::new();
    let mut result = HashMap::new();
    for function in &lowered_module.callable_defs {
        let Some(origin) = &function.scope.source_origin else {
            continue;
        };
        if origin.role != soac_core::block_py::CallableSourceRole::SourceFunction {
            continue;
        }
        let fact = verified
            .type_facts()
            .facts()
            .functions
            .iter()
            .find(|fact| fact.identity == origin.definition)
            .ok_or_else(|| {
                crate::strict_runtime_unavailable(
                    py,
                    "strict callable has no matching authenticated source identity",
                )
            })?;
        let first_offset = fact
            .decorators
            .iter()
            .map(|decorator| decorator.expression_range.start)
            .chain([fact.identity.source_range.start])
            .min()
            .unwrap();
        let first_line = line_starts.partition_point(|start| *start <= first_offset as usize);
        let mut candidates = native_codes.iter().enumerate().filter_map(|(index, code)| {
            match strict_native_code_matches(code, function, fact, &line_starts, first_line) {
                Ok(true) => Some(Ok((index, code))),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            }
        });
        let Some((index, candidate)) = candidates.next().transpose()? else {
            // CPython may omit an unreachable definition. If it is ever
            // instantiated, the strict function constructor rejects the missing
            // native witness instead of attaching a name-based replacement.
            continue;
        };
        if candidates.next().transpose()?.is_some() || !matched.insert(candidate.as_ptr() as usize)
        {
            return Err(crate::strict_runtime_unavailable(
                py,
                "ambiguous strict source-to-native-code mapping",
            ));
        }
        result.insert(function.function_id, candidate.clone().unbind());
        source_nodes.insert(origin.definition.clone(), index);
    }
    for function in &lowered_module.callable_defs {
        let Some(origin) = function.scope.source_origin.as_ref().filter(|origin| {
            origin.role == soac_core::block_py::CallableSourceRole::TypeParameterScope
        }) else {
            continue;
        };
        let projection = function
            .scope
            .type_parameter_scope
            .as_ref()
            .ok_or_else(|| {
                crate::strict_runtime_unavailable(
                    py,
                    "generic scope has no explicit native projection",
                )
            })?;
        if projection.native_range != origin.definition.source_range
            || projection.native_header_range.start < projection.native_range.start
            || projection.native_header_range.start >= projection.native_range.end
            || projection.native_header_range.end != projection.native_range.end
            || !matches!(
                origin.definition.definition_kind,
                soac_contracts::DefinitionKind::Function
                    | soac_contracts::DefinitionKind::Class
                    | soac_contracts::DefinitionKind::TypeAlias
            )
        {
            return Err(crate::strict_runtime_unavailable(
                py,
                "generic scope declaration mismatch",
            ));
        }
        let mut candidates = Vec::new();
        for code in &native_codes {
            if code.getattr("co_name")?.extract::<String>()? == function.names.display_name
                && code.getattr("co_qualname")?.extract::<String>()? == projection.native_qualname
                && code.getattr("co_firstlineno")?.extract::<u32>()? == projection.native_first_line
                && strict_native_type_expression_range_matches(
                    code,
                    &projection.native_header_range,
                    &line_starts,
                )?
            {
                candidates.push(code);
            }
        }
        let candidate = match candidates.as_slice() {
            [] => continue,
            [candidate] => *candidate,
            _ => {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "ambiguous native generic scope occurrence",
                ));
            }
        };
        let captures = function
            .public_storage_layout()
            .map_or_else(Vec::new, |layout| {
                layout
                    .freevars
                    .iter()
                    .map(|slot| slot.logical_name.clone())
                    .collect::<Vec<_>>()
            });
        let names = candidate.getattr("co_varnames")?.extract::<Vec<String>>()?;
        let native_captures = candidate.getattr("co_freevars")?.extract::<Vec<String>>()?;
        if candidate.getattr("co_argcount")?.extract::<usize>()? != projection.inputs.len()
            || candidate
                .getattr("co_posonlyargcount")?
                .extract::<usize>()?
                != 0
            || candidate.getattr("co_kwonlyargcount")?.extract::<usize>()? != 0
            || names.len() < projection.inputs.len()
            || names
                .iter()
                .zip(&projection.inputs)
                .any(|(name, input)| name != input.kind.native_parameter_name())
            || native_captures != captures
            || !matched.insert(candidate.as_ptr() as usize)
        {
            return Err(crate::strict_runtime_unavailable(
                py,
                format!(
                    "generic scope native projection differs for {}: native captures {:?}, lowered {:?}",
                    origin.definition.lexical_qualname, native_captures, captures,
                ),
            ));
        }
        result.insert(function.function_id, candidate.clone().unbind());
    }
    for function in &lowered_module.callable_defs {
        let Some(origin) = function.scope.source_origin.as_ref().filter(|origin| {
            origin.role == soac_core::block_py::CallableSourceRole::AnnotationProvider
        }) else {
            continue;
        };
        let projection = function.scope.annotation_provider.as_ref().ok_or_else(|| {
            crate::strict_runtime_unavailable(
                py,
                "strict annotation provider has no explicit native projection",
            )
        })?;
        let type_expression =
            projection.kind != soac_core::block_py::AnnotationProviderKind::Dictionary;
        let candidate = if type_expression {
            let range = projection.native_range.as_ref().ok_or_else(|| {
                crate::strict_runtime_unavailable(py, "type evaluator has no native source span")
            })?;
            let definition = &origin.definition;
            if !matches!(
                definition.definition_kind,
                soac_contracts::DefinitionKind::TypeAlias
                    | soac_contracts::DefinitionKind::Parameter
            ) || range.start < definition.source_range.start
                || range.end > definition.source_range.end
                || range.start >= range.end
            {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "type evaluator span is outside its declaration",
                ));
            }
            let mut candidates = Vec::new();
            for code in &native_codes {
                if code.getattr("co_name")?.extract::<String>()? == function.names.display_name
                    && code.getattr("co_qualname")?.extract::<String>()? == function.names.qualname
                    && code.getattr("co_firstlineno")?.extract::<u32>()?
                        == projection.native_first_line
                    && strict_native_type_expression_range_matches(code, range, &line_starts)?
                {
                    candidates.push(code);
                }
            }
            match candidates.as_slice() {
                [] => continue,
                [code] => *code,
                _ => {
                    return Err(crate::strict_runtime_unavailable(
                        py,
                        "ambiguous native type expression occurrence",
                    ));
                }
            }
        } else {
            let first_line = projection.native_first_line;
            let parent = match origin.definition.definition_kind {
                soac_contracts::DefinitionKind::Module => 0,
                soac_contracts::DefinitionKind::Function => {
                    let Some(index) = source_nodes.get(&origin.definition) else {
                        continue;
                    };
                    native_parents[*index].ok_or_else(|| {
                        crate::strict_runtime_unavailable(
                            py,
                            "function annotation target has no native parent",
                        )
                    })?
                }
                soac_contracts::DefinitionKind::Class => {
                    let fact = verified
                        .type_facts()
                        .facts()
                        .classes
                        .iter()
                        .find(|class| class.identity == origin.definition)
                        .ok_or_else(|| {
                            crate::strict_runtime_unavailable(
                                py,
                                "class annotation target has no source definition",
                            )
                        })?;
                    let offset = fact
                        .decorators
                        .iter()
                        .map(|decorator| decorator.expression_range.start)
                        .chain([fact.identity.source_range.start])
                        .min()
                        .unwrap();
                    let class_line = line_starts.partition_point(|start| *start <= offset as usize);
                    let mut candidates = Vec::new();
                    for (index, code) in native_codes.iter().enumerate() {
                        if code.getattr("co_qualname")?.extract::<String>()?
                            == fact.identity.lexical_qualname
                            && code.getattr("co_firstlineno")?.extract::<usize>()? == class_line
                            && code.getattr("co_argcount")?.extract::<usize>()? == 0
                        {
                            candidates.push(index);
                        }
                    }
                    match candidates.as_slice() {
                        [] => continue,
                        [index] => *index,
                        _ => {
                            return Err(crate::strict_runtime_unavailable(
                                py,
                                "ambiguous native class annotation parent",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(crate::strict_runtime_unavailable(
                        py,
                        "unsupported native annotation target kind",
                    ));
                }
            };
            let mut candidates = Vec::new();
            for (index, code) in native_codes.iter().enumerate() {
                if native_parents[index] == Some(parent)
                    && code.getattr("co_name")?.extract::<String>()? == "__annotate__"
                    && code.getattr("co_firstlineno")?.extract::<u32>()? == first_line
                {
                    candidates.push(code);
                }
            }
            match candidates.as_slice() {
                [] => continue,
                [code] => *code,
                _ => {
                    return Err(crate::strict_runtime_unavailable(
                        py,
                        "ambiguous native annotation provider occurrence",
                    ));
                }
            }
        };
        let captures = function
            .public_storage_layout()
            .map_or_else(Vec::new, |layout| {
                layout
                    .freevars
                    .iter()
                    .map(|slot| slot.logical_name.clone())
                    .collect::<Vec<_>>()
            });
        let native_captures = candidate.getattr("co_freevars")?.extract::<Vec<String>>()?;
        if candidate.getattr("co_argcount")?.extract::<usize>()? != 1
            || candidate
                .getattr("co_posonlyargcount")?
                .extract::<usize>()?
                != 1
            || candidate.getattr("co_kwonlyargcount")?.extract::<usize>()? != 0
            || candidate
                .getattr("co_varnames")?
                .cast::<PyTuple>()?
                .get_item(0)?
                .extract::<String>()?
                != projection.kind.parameter_name()
            || candidate.getattr("co_qualname")?.extract::<String>()? != function.names.qualname
            || native_captures != captures
        {
            return Err(crate::strict_runtime_unavailable(
                py,
                format!(
                    "annotation provider native projection differs for {}: native captures {:?}, lowered {:?}",
                    origin.definition.lexical_qualname, native_captures, captures
                ),
            ));
        }
        if !matched.insert(candidate.as_ptr() as usize) {
            return Err(crate::strict_runtime_unavailable(
                py,
                "native annotation provider has multiple source owners",
            ));
        }
        result.insert(function.function_id, candidate.clone().unbind());
    }
    let mut generator_expression_codes = HashMap::new();
    for function in &lowered_module.callable_defs {
        let Some(projection) = &function.scope.generator_expression_code else {
            continue;
        };
        if function.scope.source_origin.is_some()
            || projection.expression_range.end > verified.source().len() as u32
        {
            return Err(crate::strict_runtime_unavailable(
                py,
                "invalid generator code-exposure projection",
            ));
        }
        let Some(index) = match_native_generator_expression_code(
            &native_codes,
            projection,
            function.lowered_kind(),
            &line_starts,
        )?
        else {
            // Unreachable expressions can be absent from native compilation.
            // Invocation with a required but missing exposure fails explicitly.
            continue;
        };
        let candidate = &native_codes[index];
        if !matched.insert(candidate.as_ptr() as usize) {
            return Err(crate::strict_runtime_unavailable(
                py,
                "generator code has multiple source projections",
            ));
        }
        generator_expression_codes.insert(function.function_id, candidate.clone().unbind());
    }
    crate::AuthenticatedCodeCatalog::from_compiled(
        py,
        verified,
        lowered_module,
        &native_root,
        result,
        generator_expression_codes,
    )
}

/// Check the populated-table precondition before native address lookups.
/// PyCode_Addr2Location assumes an actual table entry exists: with an empty
/// co_linetable its failed range search can retreat before the table. A header
/// synthesized from that memory is neither safe nor a source witness. The
/// caller otherwise supplies the exact privately compiled native code tree.
fn strict_native_location_code_length(code: &Bound<'_, PyAny>) -> PyResult<Option<i32>> {
    unsafe extern "C" {
        fn PyCode_GetCode(code: *mut ffi::PyCodeObject) -> *mut ffi::PyObject;
    }
    if unsafe { ffi::Py_TYPE(code.as_ptr()) } != ptr::addr_of_mut!(ffi::PyCode_Type) {
        return Ok(None);
    }
    let locations = code
        .getattr("co_linetable")?
        .cast_into::<pyo3::types::PyBytes>()?;
    if locations.as_bytes().is_empty() {
        return Ok(None);
    }
    let bytecode = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(code.py(), PyCode_GetCode(code.as_ptr().cast()))?
    };
    let length = unsafe { ffi::PyBytes_Size(bytecode.as_ptr()) };
    if length < 0 {
        return Err(PyErr::fetch(code.py()));
    }
    let length = i32::try_from(length).map_err(|_| {
        crate::strict_runtime_unavailable(code.py(), "native position bytecode is too large")
    })?;
    Ok((length != 0).then_some(length))
}

/// Code exposure is separate from callable admission. An exact outer-iterable
/// location distinguishes even same-line genexprs with identical native names.
/// The caller has already validated every node of the privately compiled tree.
fn match_native_generator_expression_code(
    native_codes: &[Bound<'_, PyAny>],
    projection: &soac_core::block_py::GeneratorExpressionCode,
    kind: &FunctionKind,
    line_starts: &[usize],
) -> PyResult<Option<usize>> {
    let mut matched = None;
    for (index, code) in native_codes.iter().enumerate() {
        if strict_native_generator_expression_matches(code, projection, kind, line_starts)? {
            if matched.replace(index).is_some() {
                return Err(crate::strict_runtime_unavailable(
                    code.py(),
                    "ambiguous original generator-expression code",
                ));
            }
        }
    }
    Ok(matched)
}

fn strict_native_generator_expression_matches(
    code: &Bound<'_, PyAny>,
    projection: &soac_core::block_py::GeneratorExpressionCode,
    kind: &FunctionKind,
    line_starts: &[usize],
) -> PyResult<bool> {
    unsafe extern "C" {
        fn PyCode_Addr2Location(
            code: *mut ffi::PyCodeObject,
            offset: c_int,
            start_line: *mut c_int,
            start_column: *mut c_int,
            end_line: *mut c_int,
            end_column: *mut c_int,
        ) -> c_int;
    }
    let expected_flag = match kind {
        FunctionKind::Generator => ffi::CO_GENERATOR,
        FunctionKind::AsyncGenerator => ffi::CO_ASYNC_GENERATOR,
        _ => return Ok(false),
    };
    let expression = &projection.expression_range;
    let iterable = &projection.iterable_range;
    if expression.start >= expression.end
        || iterable.start >= iterable.end
        || iterable.start < expression.start
        || iterable.end > expression.end
        || unsafe { ffi::Py_TYPE(code.as_ptr()) } != ptr::addr_of_mut!(ffi::PyCode_Type)
    {
        return Ok(false);
    }
    let first_line = line_starts.partition_point(|start| *start <= expression.start as usize);
    let flags = code.getattr("co_flags")?.extract::<i32>()?;
    if code.getattr("co_name")?.extract::<String>()? != "<genexpr>"
        || code.getattr("co_firstlineno")?.extract::<usize>()? != first_line
        || flags & (ffi::CO_GENERATOR | ffi::CO_COROUTINE | ffi::CO_ASYNC_GENERATOR)
            != expected_flag
        || flags & (ffi::CO_VARARGS | ffi::CO_VARKEYWORDS) != 0
        || code.getattr("co_argcount")?.extract::<usize>()? != 1
        || code.getattr("co_posonlyargcount")?.extract::<usize>()? != 0
        || code.getattr("co_kwonlyargcount")?.extract::<usize>()? != 0
        || code
            .getattr("co_varnames")?
            .cast_into::<PyTuple>()?
            .get_item(0)?
            .extract::<String>()?
            != ".0"
    {
        return Ok(false);
    }
    let py = code.py();
    let Some(length) = strict_native_location_code_length(code)? else {
        return Ok(false);
    };
    let offset = |line: i32, column: i32| -> Option<usize> {
        let line = usize::try_from(line).ok()?.checked_sub(1)?;
        line_starts
            .get(line)?
            .checked_add(usize::try_from(column).ok()?)
    };
    let mut first_span = None;
    for instruction in (0..length).step_by(2) {
        let (mut line, mut column, mut end_line, mut end_column) = (0, 0, 0, 0);
        if unsafe {
            PyCode_Addr2Location(
                code.as_ptr().cast(),
                instruction,
                &mut line,
                &mut column,
                &mut end_line,
                &mut end_column,
            )
        } == 0
        {
            return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
                crate::strict_runtime_unavailable(py, "native generator source position is absent")
            } else {
                PyErr::fetch(py)
            });
        }
        let (Some(start), Some(end)) = (offset(line, column), offset(end_line, end_column)) else {
            continue;
        };
        if start == end {
            continue;
        }
        if start < expression.start as usize || end > expression.end as usize || start > end {
            return Ok(false);
        }
        first_span.get_or_insert((start, end));
    }
    Ok(first_span == Some((iterable.start as usize, iterable.end as usize)))
}

/// Native annotation-scope prologues carry the original alias/expression span.
/// Match that exact span, not a generated helper name or a shared line number;
/// bounds and defaults can have the same public name on the same source line.
fn strict_native_type_expression_range_matches(
    code: &Bound<'_, PyAny>,
    range: &soac_contracts::SourceRange,
    line_starts: &[usize],
) -> PyResult<bool> {
    unsafe extern "C" {
        fn PyCode_Addr2Location(
            code: *mut ffi::PyCodeObject,
            offset: c_int,
            start_line: *mut c_int,
            start_column: *mut c_int,
            end_line: *mut c_int,
            end_column: *mut c_int,
        ) -> c_int;
    }
    let py = code.py();
    let Some(length) = strict_native_location_code_length(code)? else {
        return Ok(false);
    };
    let offset = |line: i32, column: i32| -> Option<usize> {
        let line = usize::try_from(line).ok()?.checked_sub(1)?;
        let column = usize::try_from(column).ok()?;
        line_starts.get(line)?.checked_add(column)
    };
    let (start, end) = (range.start as usize, range.end as usize);
    let mut exact_scope = false;
    for instruction in (0..length).step_by(2) {
        let (mut line, mut column, mut end_line, mut end_column) = (0, 0, 0, 0);
        if unsafe {
            PyCode_Addr2Location(
                code.as_ptr().cast(),
                instruction,
                &mut line,
                &mut column,
                &mut end_line,
                &mut end_column,
            )
        } == 0
        {
            return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
                crate::strict_runtime_unavailable(
                    py,
                    "native type expression source position is absent",
                )
            } else {
                PyErr::fetch(py)
            });
        }
        let (Some(native_start), Some(native_end)) =
            (offset(line, column), offset(end_line, end_column))
        else {
            continue;
        };
        if native_start == native_end {
            continue;
        }
        if native_start < start || native_end > end {
            return Ok(false);
        }
        exact_scope |= native_start == start && native_end == end;
    }
    Ok(exact_scope)
}

fn strict_native_code_matches(
    code: &Bound<'_, PyAny>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    fact: &soac_contracts::FunctionTypeFact,
    line_starts: &[usize],
    first_line: usize,
) -> PyResult<bool> {
    unsafe extern "C" {
        fn PyCode_Addr2Location(
            code: *mut ffi::PyCodeObject,
            offset: c_int,
            start_line: *mut c_int,
            start_column: *mut c_int,
            end_line: *mut c_int,
            end_column: *mut c_int,
        ) -> c_int;
    }
    // The source catalog validates the signed lexical identity independently
    // and derives this exact native projection from the original AST. Native
    // lambda/genexpr nesting is not the signed lexical naming convention.
    if code.getattr("co_qualname")?.extract::<String>()? != function.names.qualname
        || code.getattr("co_firstlineno")?.extract::<usize>()? != first_line
    {
        return Ok(false);
    }
    use soac_core::block_py::ParamKind;
    let count = |kind| {
        function
            .params
            .iter()
            .filter(|parameter| parameter.kind == kind)
            .count()
    };
    let positional = count(ParamKind::PosOnly) + count(ParamKind::Any);
    if code.getattr("co_argcount")?.extract::<usize>()? != positional
        || code.getattr("co_posonlyargcount")?.extract::<usize>()? != count(ParamKind::PosOnly)
        || code.getattr("co_kwonlyargcount")?.extract::<usize>()? != count(ParamKind::KwOnly)
    {
        return Ok(false);
    }
    let flags = code.getattr("co_flags")?.extract::<i32>()?;
    let native_kind = if flags & ffi::CO_ASYNC_GENERATOR != 0 {
        FunctionKind::AsyncGenerator
    } else if flags & ffi::CO_COROUTINE != 0 {
        FunctionKind::Coroutine
    } else if flags & ffi::CO_GENERATOR != 0 {
        FunctionKind::Generator
    } else {
        FunctionKind::Function
    };
    if native_kind != *function.lowered_kind() {
        return Ok(false);
    }
    if (flags & 4 != 0) != (count(ParamKind::VarArg) != 0)
        || (flags & 8 != 0) != (count(ParamKind::KwArg) != 0)
    {
        return Ok(false);
    }
    let names = code.getattr("co_varnames")?.cast_into::<PyTuple>()?;
    let expected_names = function
        .params
        .iter()
        .filter(|p| matches!(p.kind, ParamKind::PosOnly | ParamKind::Any))
        .chain(
            function
                .params
                .iter()
                .filter(|p| p.kind == ParamKind::KwOnly),
        )
        .chain(
            function
                .params
                .iter()
                .filter(|p| p.kind == ParamKind::VarArg),
        )
        .chain(
            function
                .params
                .iter()
                .filter(|p| p.kind == ParamKind::KwArg),
        );
    for (index, expected) in expected_names.enumerate() {
        if names.get_item(index)?.extract::<String>()? != expected.name {
            return Ok(false);
        }
    }
    let Some(length) = strict_native_location_code_length(code)? else {
        return Ok(false);
    };
    let offset = |line: i32, column: i32| -> Option<usize> {
        let line = usize::try_from(line).ok()?.checked_sub(1)?;
        let column = usize::try_from(column).ok()?;
        line_starts.get(line)?.checked_add(column)
    };
    let start = fact.identity.source_range.start as usize;
    let end = fact.identity.source_range.end as usize;
    let mut has_source_position = false;
    let mut has_header_anchor = false;
    for instruction in (0..length).step_by(2) {
        let (mut line, mut column, mut end_line, mut end_column) = (0, 0, 0, 0);
        if unsafe {
            PyCode_Addr2Location(
                code.as_ptr().cast(),
                instruction,
                &mut line,
                &mut column,
                &mut end_line,
                &mut end_column,
            )
        } == 0
        {
            return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
                crate::strict_runtime_unavailable(
                    code.py(),
                    "strict native source position is unavailable",
                )
            } else {
                PyErr::fetch(code.py())
            });
        }
        let (Some(native_start), Some(native_end)) =
            (offset(line, column), offset(end_line, end_column))
        else {
            continue;
        };
        if native_start == native_end {
            has_header_anchor |= line == end_line
                && usize::try_from(line).ok() == Some(first_line)
                && column == 0
                && end_column == 0;
            continue;
        }
        // Nonempty opcode spans disambiguate multiple lambdas on one line.
        // The first RESUME can refer to a decorator line, which belongs to the
        // same authenticated function but precedes its definition range.
        if instruction == 0 && native_start < start {
            continue;
        }
        if native_start < start || native_end > end {
            return Ok(false);
        }
        has_source_position = true;
    }
    // Named bodies containing only declarations/docstrings can have no
    // nonempty native opcode span. Their real linetable still anchors the
    // exact first line (including an earlier decorator). The caller separately
    // authenticates the complete native code tree and rejects reused/ambiguous
    // matches; the name, line, signature and kind checks above remain required.
    // Lambdas cannot use this fallback: same-line lambdas need their body span.
    Ok(has_source_position
        || (fact.identity.definition_kind == soac_contracts::DefinitionKind::Function
            && has_header_anchor))
}

pub fn build_shared_state_for_inspection(
    py: Python<'_>,
    lowered_module: BlockPyModule<BlockPyModuleShape>,
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
    lowered_module: BlockPyModule<BlockPyModuleShape>,
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
        None,
        HashMap::new(),
    )
}

pub fn build_shared_state_for_inspection_with_placeholder_constants(
    py: Python<'_>,
    lowered_module: BlockPyModule<BlockPyModuleShape>,
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
    Ok(Arc::new(SharedModuleState {
        strict_module: None,
        strict_execution: None,
        late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
            &lowered_module,
            module_name,
        ),
        lowered_module,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        source_hash: 0,
        codegen_constants,
        storage_instance_key: allocate_shared_module_state_storage_key(),
        function_index_by_id,
        function_templates: Mutex::new(HashMap::new()),
        original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
            HashMap::new(),
        ),
        module_constant_objs,
        runtime_name_cache: build_runtime_name_cache(),
        counter_slots_by_id,
        counter_values,
        top_value_counters,
        deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
        counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
    }))
}

pub fn build_shared_state_for_inspection_with_placeholder_constants_and_source_hash(
    py: Python<'_>,
    lowered_module: BlockPyModule<BlockPyModuleShape>,
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
    Ok(Arc::new(SharedModuleState {
        strict_module: None,
        strict_execution: None,
        late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
            &lowered_module,
            module_name,
        ),
        lowered_module,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        source_hash,
        codegen_constants,
        storage_instance_key: allocate_shared_module_state_storage_key(),
        function_index_by_id,
        function_templates: Mutex::new(HashMap::new()),
        original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
            HashMap::new(),
        ),
        module_constant_objs,
        runtime_name_cache: build_runtime_name_cache(),
        counter_slots_by_id,
        counter_values,
        top_value_counters,
        deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
        counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
    }))
}

fn build_shared_state_for_inspection_with_original_code(
    py: Python<'_>,
    lowered_module: BlockPyModule<BlockPyModuleShape>,
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
        None,
        original_code_by_function_id,
    )
}

pub fn build_shared_state_for_inspection_with_original_code_and_source_hash(
    py: Python<'_>,
    lowered_module: BlockPyModule<BlockPyModuleShape>,
    module_name: &str,
    package_name: &str,
    source_hash: u64,
    _source: Option<&str>,
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
    let module_constant_objs = build_module_constant_objects(py, &codegen_constants, module_name)?;
    Ok(Arc::new(SharedModuleState {
        strict_module: None,
        strict_execution: None,
        late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
            &lowered_module,
            module_name,
        ),
        lowered_module,
        module_name: module_name.to_string(),
        package_name: package_name.to_string(),
        source_hash,
        codegen_constants,
        storage_instance_key: allocate_shared_module_state_storage_key(),
        function_index_by_id,
        function_templates: Mutex::new(HashMap::new()),
        original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
            original_code_by_function_id,
        ),
        module_constant_objs,
        runtime_name_cache: build_runtime_name_cache(),
        counter_slots_by_id,
        counter_values,
        top_value_counters,
        deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
        counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
    }))
}

#[cfg(test)]
pub(crate) fn build_shared_state_for_testing(
    py: Python<'_>,
    lowered_module: BlockPyModule<BlockPyModuleShape>,
    module_name: &str,
    package_name: &str,
) -> PyResult<Arc<SharedModuleState>> {
    build_shared_state_for_inspection(py, lowered_module, module_name, package_name)
}

fn build_function_index_by_id(
    module: &BlockPyModule<BlockPyModuleShape>,
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

#[repr(C)]
struct SoacExtModuleState {
    initialized: bool,
    implementation: MaybeUninit<ModuleImplementation>,
}

enum ModuleImplementation {
    Soac {
        shared_state: Arc<SharedModuleState>,
        strict_runtime: Option<crate::StrictModuleRuntimeState>,
    },
    Interpreter(crate::strict_interpreter::InterpreterModuleState),
}

impl SoacExtModuleState {
    unsafe fn init(
        &mut self,
        py: Python<'_>,
        compile_session: &Arc<crate::session::CompileSession>,
        lowered_module: BlockPyModule<BlockPyModuleShape>,
        original_code_by_function_id: crate::AuthenticatedCodeCatalog,
        module_name: String,
        package_name: String,
        source_hash: u64,
        strict_module: Option<Arc<crate::VerifiedStrictModule>>,
        strict_runtime: Option<crate::StrictModuleRuntimeState>,
    ) -> PyResult<()> {
        if self.initialized {
            return Err(PyRuntimeError::new_err(
                "transformed module state was unexpectedly initialized twice",
            ));
        }
        let function_index_by_id = build_function_index_by_id(&lowered_module)?;
        let (counter_slots_by_id, counter_values, top_value_counters) =
            build_counter_storage(&lowered_module.counter_defs)?;
        let codegen_constants = ModuleCodegenConstants::collect_from_module(&lowered_module);
        let module_constant_objs =
            build_module_constant_objects(py, &codegen_constants, module_name.as_str())?;
        let shared_state = Arc::new(SharedModuleState {
            strict_module,
            strict_execution: strict_runtime
                .as_ref()
                .map(crate::StrictModuleRuntimeState::execution_ref),
            late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
                &lowered_module,
                module_name.as_str(),
            ),
            lowered_module,
            module_name,
            package_name,
            source_hash,
            codegen_constants,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            function_index_by_id,
            function_templates: Mutex::new(HashMap::new()),
            original_code_by_function_id:
                crate::strict_admission::OriginalCodeStorage::Authenticated(
                    original_code_by_function_id,
                ),
            module_constant_objs,
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id,
            counter_values,
            top_value_counters,
            deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
            counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
        });
        compile_session
            .retain_shared_module_state(shared_state.clone())
            .map_err(PyRuntimeError::new_err)?;
        self.implementation.write(ModuleImplementation::Soac {
            shared_state,
            strict_runtime,
        });
        self.initialized = true;
        Ok(())
    }

    unsafe fn clear(&mut self) {
        if !self.initialized {
            return;
        }
        // Publish terminal native state before any decref can reenter it.
        self.initialized = false;
        let implementation = unsafe { self.implementation.assume_init_read() };
        let ModuleImplementation::Soac {
            shared_state,
            strict_runtime,
        } = implementation
        else {
            if let ModuleImplementation::Interpreter(state) = implementation {
                unsafe { state.release_from_native(Python::assume_attached()) };
            }
            return;
        };
        // A module wrapper may die while an escaped function still owns its
        // globals. Only actual dictionary teardown terminates a sealed policy;
        // dropping an unfinished execution still fails it closed.
        if let Some(strict) = strict_runtime {
            unsafe { strict.release_from_native(Python::assume_attached()) };
        }
        shared_state.append_specialization_runtime_log();
        let mut flushed = true;
        if let Some(path) = counter_dump_file_from_env() {
            if let Err(err) = shared_state.flush_counter_dump_file_once(path.as_path()) {
                flushed = false;
                eprintln!(
                    "[soac counters] failed to append counter dump to {}: {err}",
                    path.display()
                );
            }
        }
        if flushed && let Err(err) = shared_state.mark_counter_dump_module_cleared() {
            eprintln!("[soac counters] failed to mark cleared module: {err}");
        }
        drop(shared_state);
    }

    unsafe fn data(&self) -> PyResult<SoacExtModuleDataRef<'_>> {
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "missing transformed-module lowering data in module state",
            ));
        }
        match unsafe { self.implementation.assume_init_ref() } {
            ModuleImplementation::Soac {
                shared_state,
                strict_runtime,
            } => Ok(SoacExtModuleDataRef {
                shared_state,
                strict_runtime: strict_runtime.as_ref(),
            }),
            ModuleImplementation::Interpreter(_) => Err(PyRuntimeError::new_err(
                "the CPython interpreter module has no SOAC lowering state",
            )),
        }
    }

    unsafe fn clone_shared_state(&self) -> PyResult<Arc<SharedModuleState>> {
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "missing transformed-module lowering data in module state",
            ));
        }
        match unsafe { self.implementation.assume_init_ref() } {
            ModuleImplementation::Soac { shared_state, .. } => Ok(shared_state.clone()),
            ModuleImplementation::Interpreter(_) => Err(PyRuntimeError::new_err(
                "the CPython interpreter module has no SOAC shared state",
            )),
        }
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
    function: &BlockPyFunction<BlockPyModuleShape>,
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
        jit_strict_sealed_field_site_count = stats.strict_sealed_field_site_count,
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
        return unsafe { record_preseeded_split_key_layout(type_obj) };
    }
    if unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) } != 0 {
        unsafe { ffi::PyErr_Clear() };
        return Ok(());
    }
    Err(())
}

#[repr(C)]
struct RawPyDictKeysObjectForProfile {
    dk_refcnt: ffi::Py_ssize_t,
    dk_log2_size: u8,
    dk_log2_index_bytes: u8,
    dk_kind: u8,
    dk_version: u32,
    dk_usable: ffi::Py_ssize_t,
    dk_nentries: ffi::Py_ssize_t,
}

#[repr(C)]
struct RawPyDictUnicodeEntryForProfile {
    me_key: *mut ffi::PyObject,
    me_value: *mut ffi::PyObject,
}

struct PreseededTypeKeyLayout {
    owner_weakref: Py<PyAny>,
    cached_keys: usize,
    initial_capacity: usize,
    entries: Vec<(String, u32)>,
}

unsafe fn record_preseeded_split_key_layout(type_obj: *mut ffi::PyObject) -> Result<(), ()> {
    const DICT_KEYS_SPLIT: u8 = 2;
    const SHARED_KEYS_MAX_SIZE: usize = 30;

    if cfg!(Py_GIL_DISABLED) || unsafe { ffi::PyType_Check(type_obj) } == 0 {
        return Ok(());
    }
    let owner_type = type_obj.cast::<ffi::PyTypeObject>();
    if unsafe { (*owner_type).tp_flags & ffi::Py_TPFLAGS_HEAPTYPE } == 0 {
        return Ok(());
    }

    let keys = unsafe {
        (*owner_type.cast::<ffi::PyHeapTypeObject>())
            .ht_cached_keys
            .cast::<RawPyDictKeysObjectForProfile>()
    };
    if keys.is_null() || unsafe { (*keys).dk_kind } != DICT_KEYS_SPLIT {
        return Ok(());
    }

    let Ok(nentries) = usize::try_from(unsafe { (*keys).dk_nentries }) else {
        return Ok(());
    };
    if nentries == 0 || nentries > SHARED_KEYS_MAX_SIZE {
        return Ok(());
    }
    let Ok(usable) = usize::try_from(unsafe { (*keys).dk_usable }) else {
        return Ok(());
    };
    let Some(initial_capacity) = usable.checked_add(nentries) else {
        return Ok(());
    };
    if initial_capacity > SHARED_KEYS_MAX_SIZE {
        return Ok(());
    }
    let Some(bucket_count) = 1usize.checked_shl(u32::from(unsafe { (*keys).dk_log2_size })) else {
        return Ok(());
    };
    let Some(index_bytes) = 1usize.checked_shl(u32::from(unsafe { (*keys).dk_log2_index_bytes }))
    else {
        return Ok(());
    };
    if nentries > bucket_count || index_bytes < bucket_count {
        return Ok(());
    }
    let Some(entries_offset) =
        std::mem::size_of::<RawPyDictKeysObjectForProfile>().checked_add(index_bytes)
    else {
        return Ok(());
    };
    let entries = unsafe {
        keys.cast::<u8>()
            .add(entries_offset)
            .cast::<RawPyDictUnicodeEntryForProfile>()
    };
    let py = unsafe { Python::assume_attached() };
    let mut existing = Vec::with_capacity(nentries);
    for index in 0..nentries {
        let key = unsafe { (*entries.add(index)).me_key };
        if key.is_null() || unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            continue;
        }
        let key = match unsafe { Bound::<PyAny>::from_borrowed_ptr(py, key) }.extract::<String>() {
            Ok(key) => key,
            Err(error) => {
                error.restore(py);
                return Err(());
            }
        };
        existing.push((key, index as u32));
    }
    if existing.is_empty() {
        return Ok(());
    }

    let owner_weakref = unsafe { crate::PyWeakref_NewRef(type_obj, ptr::null_mut()) };
    if owner_weakref.is_null() {
        return Err(());
    }
    let owner_weakref = unsafe { Bound::<PyAny>::from_owned_ptr(py, owner_weakref) }.unbind();
    let mut registry = profile_type_registry().lock().map_err(|_| {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"profile type registry lock was poisoned".as_ptr(),
            )
        };
    })?;
    registry.preseeded_layouts.push(PreseededTypeKeyLayout {
        owner_weakref,
        cached_keys: keys as usize,
        initial_capacity,
        entries: existing,
    });
    Ok(())
}

#[derive(Default)]
struct ProfileTypeRegistry {
    by_type: HashMap<usize, u64>,
    entries_by_id: HashMap<u64, CounterDumpTypeKey>,
    preseeded_layouts: Vec<PreseededTypeKeyLayout>,
}

impl ProfileTypeRegistry {
    fn id_for_type(&mut self, owner_ptr: usize, key: CounterDumpTypeKey) -> PyResult<u64> {
        if let Some(type_id) = self.by_type.get(&owner_ptr).copied() {
            if self.entries_by_id.get(&type_id) == Some(&key) {
                return Ok(type_id);
            }
            // Watched owners are weakly held, so a later heap type can reuse
            // the same address after the original class has been collected.
            self.by_type.remove(&owner_ptr);
        }
        let type_id = stable_profile_type_id(&key);
        if let Some(existing) = self.entries_by_id.get(&type_id)
            && existing != &key
        {
            return Err(PyRuntimeError::new_err(format!(
                "stable profile type id collision for {}.{} and {}.{}",
                existing.module_name, existing.qualname, key.module_name, key.qualname
            )));
        }
        self.by_type.insert(owner_ptr, type_id);
        self.entries_by_id.entry(type_id).or_insert(key);
        Ok(type_id)
    }

    fn entries_for_ids(&self, used_ids: &HashSet<u64>) -> Vec<CounterDumpTypeTableEntry> {
        let mut entries = self
            .entries_by_id
            .iter()
            .filter(|(type_id, _)| used_ids.contains(type_id))
            .map(|(type_id, key)| CounterDumpTypeTableEntry {
                type_id: *type_id,
                key: key.clone(),
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.type_id);
        entries
    }
}

fn stable_profile_type_id(key: &CounterDumpTypeKey) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in key
        .module_name
        .as_bytes()
        .iter()
        .chain(std::iter::once(&0))
        .chain(key.qualname.as_bytes().iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash.max(1)
}

static PROFILE_TYPE_REGISTRY: OnceLock<Mutex<ProfileTypeRegistry>> = OnceLock::new();

fn profile_type_registry() -> &'static Mutex<ProfileTypeRegistry> {
    PROFILE_TYPE_REGISTRY.get_or_init(|| Mutex::new(ProfileTypeRegistry::default()))
}

fn snapshot_type_key_layout_events(
    module_name: &str,
) -> (
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
    snapshot_type_key_layout_events_bound(events.as_any(), module_name).unwrap_or_default()
}

fn snapshot_type_key_layout_events_bound(
    events: &Bound<'_, PyAny>,
    module_name: &str,
) -> PyResult<(
    Vec<CounterDumpTypeKeyLayout>,
    Vec<CounterDumpTypeTableEntry>,
)> {
    let events = events.cast::<PyList>()?;
    let mut out = Vec::new();
    let mut used_type_ids = HashSet::new();
    let mut observed_layouts = HashSet::new();
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
        if observed_layouts.insert((owner_type_id, key.clone(), index)) {
            used_type_ids.insert(owner_type_id);
            out.push(CounterDumpTypeKeyLayout {
                owner_type_id,
                key,
                index,
            });
        }
    }

    let py = events.py();
    let preseeded_layouts = {
        let registry = profile_type_registry()
            .lock()
            .map_err(|_| PyRuntimeError::new_err("profile type registry lock was poisoned"))?;
        registry
            .preseeded_layouts
            .iter()
            .map(|layout| {
                (
                    layout.owner_weakref.clone_ref(py),
                    layout.cached_keys,
                    layout.initial_capacity,
                    layout.entries.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut stale_weakrefs = HashSet::new();
    for (owner_weakref, expected_keys, initial_capacity, entries) in preseeded_layouts {
        let mut owner_ptr = ptr::null_mut();
        match unsafe { crate::PyWeakref_GetRef(owner_weakref.as_ptr(), &mut owner_ptr) } {
            1 => {}
            0 => {
                stale_weakrefs.insert(owner_weakref.as_ptr() as usize);
                continue;
            }
            _ => return Err(PyErr::fetch(py)),
        }

        let owner = unsafe { Bound::<PyAny>::from_owned_ptr(py, owner_ptr) };
        let owner_type = owner.as_ptr().cast::<ffi::PyTypeObject>();
        let current_keys = unsafe {
            (*owner_type.cast::<ffi::PyHeapTypeObject>())
                .ht_cached_keys
                .cast::<RawPyDictKeysObjectForProfile>()
        };
        if current_keys.is_null()
            || current_keys as usize != expected_keys
            || unsafe { (*current_keys).dk_kind } != 2
        {
            continue;
        }
        let (Ok(current_usable), Ok(current_nentries)) = (
            usize::try_from(unsafe { (*current_keys).dk_usable }),
            usize::try_from(unsafe { (*current_keys).dk_nentries }),
        ) else {
            continue;
        };
        let Some(current_capacity) = current_usable.checked_add(current_nentries) else {
            continue;
        };
        // Adding a split key increments nentries and decrements usable, but
        // initializing an exact-owner instance decrements only usable. This
        // distinguishes classes that were actually instantiated from abstract
        // base classes whose compiler merely preseeded their cached keys.
        if current_capacity >= initial_capacity {
            continue;
        }
        let owner_dict = unsafe { (*owner_type).tp_dict };
        if owner_dict.is_null() {
            continue;
        }
        let owner_module = unsafe { ffi::PyDict_GetItemString(owner_dict, c"__module__".as_ptr()) };
        if owner_module.is_null() || unsafe { ffi::PyUnicode_CheckExact(owner_module) } == 0 {
            continue;
        }
        let owner_module =
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, owner_module) }.extract::<String>()?;
        if owner_module != module_name {
            continue;
        }
        let owner_qualname = unsafe { ffi::PyType_GetQualName(owner_type) };
        if owner_qualname.is_null() {
            return Err(PyErr::fetch(py));
        }
        let owner_qualname = unsafe { Bound::<PyAny>::from_owned_ptr(py, owner_qualname) };
        if unsafe { ffi::PyUnicode_CheckExact(owner_qualname.as_ptr()) } == 0 {
            continue;
        }
        let type_key = CounterDumpTypeKey {
            module_name: owner_module,
            qualname: owner_qualname.extract()?,
        };
        let owner_type_id = {
            let mut registry = profile_type_registry()
                .lock()
                .map_err(|_| PyRuntimeError::new_err("profile type registry lock was poisoned"))?;
            registry.id_for_type(owner.as_ptr() as usize, type_key)?
        };
        for (key, index) in entries {
            if observed_layouts.insert((owner_type_id, key.clone(), index)) {
                used_type_ids.insert(owner_type_id);
                out.push(CounterDumpTypeKeyLayout {
                    owner_type_id,
                    key,
                    index,
                });
            }
        }
    }

    let mut registry = profile_type_registry()
        .lock()
        .map_err(|_| PyRuntimeError::new_err("profile type registry lock was poisoned"))?;
    let mut stale_layouts = Vec::new();
    if !stale_weakrefs.is_empty() {
        let layouts = std::mem::take(&mut registry.preseeded_layouts);
        for layout in layouts {
            if stale_weakrefs.contains(&(layout.owner_weakref.as_ptr() as usize)) {
                stale_layouts.push(layout);
            } else {
                registry.preseeded_layouts.push(layout);
            }
        }
    }
    let type_table = registry.entries_for_ids(&used_type_ids);
    drop(registry);
    // A DECREF can run arbitrary Python code, so release removed weakrefs
    // only after dropping the registry lock.
    drop(stale_layouts);
    Ok((out, type_table))
}

pub(crate) fn counter_dump_file_from_env() -> Option<PathBuf> {
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
    let implementation = unsafe { (*state).implementation.assume_init_ref() };
    let ModuleImplementation::Soac {
        shared_state,
        strict_runtime,
    } = implementation
    else {
        if let ModuleImplementation::Interpreter(state) = implementation {
            return unsafe { state.traverse(visit, arg) };
        }
        return 0;
    };
    if let Some(strict) = strict_runtime {
        let rc = unsafe { strict.traverse(visit, arg) };
        if rc != 0 {
            return rc;
        }
    }
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
    if unsafe {
        ffi::PyModule_Check(module.as_ptr()) != 0
            && ffi::PyModule_GetDef(module.as_ptr()) == soac_strict_module_def()
    } {
        return SoacExtModule::with_data(module, |data| {
            Ok(ModuleInfo {
                hash: data.shared_state.source_hash,
                indexed_module_keys: data.shared_state.lowered_module.global_names.clone(),
            })
        });
    }
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

    unsafe { soac_init_module_info(module, module_info) }
}

unsafe fn soac_init_module_info(
    module: *mut ffi::PyObject,
    module_info: ModuleInfo,
) -> PyResult<()> {
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

// Strict modules use the exact immutable builtin ModuleType. A mutable heap
// type or Python type cache cannot authorize permanent __dict__/__class__
// semantics. This single-phase native definition supplies the same m_state
// lifecycle without routing construction through a Python-visible type.
static mut SOAC_STRICT_MODULE_DEF: ffi::PyModuleDef = ffi::PyModuleDef {
    m_base: ffi::PyModuleDef_HEAD_INIT,
    m_name: c"_soac_ext.strict_module_state".as_ptr(),
    m_doc: ptr::null(),
    m_size: std::mem::size_of::<SoacExtModuleState>() as ffi::Py_ssize_t,
    m_methods: ptr::null_mut(),
    m_slots: ptr::null_mut(),
    m_traverse: Some(soac_ext_module_traverse),
    m_clear: Some(soac_ext_module_clear),
    m_free: Some(soac_ext_module_free),
};

fn soac_strict_module_def() -> *mut ffi::PyModuleDef {
    ptr::addr_of_mut!(SOAC_STRICT_MODULE_DEF)
}

pub(crate) fn new_strict_module<'py>(
    py: Python<'py>,
    spec: &Bound<'py, PyAny>,
    name: &str,
    package: &str,
) -> PyResult<Bound<'py, PyAny>> {
    // Evaluate spec access before creating the new native module. These public
    // attributes initialize Python-visible metadata; they do not become the
    // source/deployment identity used by the strict owner.
    let loader = spec.getattr("loader")?;
    let has_location = spec.getattr("has_location")?.is_truthy()?;
    let origin = has_location.then(|| spec.getattr("origin")).transpose()?;
    let cached = has_location.then(|| spec.getattr("cached")).transpose()?;
    let search = spec.getattr("submodule_search_locations")?;
    let module = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            ffi::PyModule_Create2(soac_strict_module_def(), ffi::PYTHON_API_VERSION),
        )?
    };
    let globals =
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, ffi::PyModule_GetDict(module.as_ptr())) }
            .cast_into::<PyDict>()?;
    globals.set_item("__name__", name)?;
    globals.set_item("__package__", package)?;
    globals.set_item("__loader__", loader)?;
    globals.set_item("__spec__", spec)?;
    if let Some(origin) = origin {
        globals.set_item("__file__", origin)?;
    }
    if let Some(cached) = cached {
        globals.set_item("__cached__", cached)?;
    }
    if !search.is_none() {
        globals.set_item("__path__", search)?;
    }
    Ok(module)
}

/// Match the ordinary exec/import insertion without normalizing an existing
/// explicit builtins mapping. This runs during the one initializer attempt.
pub fn ensure_module_builtins(globals: &Bound<'_, PyAny>) -> PyResult<()> {
    let globals = globals.cast::<PyDict>()?;
    if globals.get_item("__builtins__")?.is_some() {
        return Ok(());
    }
    let builtins = unsafe { ffi::PyEval_GetBuiltins() };
    if builtins.is_null() {
        return Err(PyRuntimeError::new_err(
            "module initialization has no native builtins",
        ));
    }
    globals.set_item("__builtins__", unsafe {
        Bound::<PyAny>::from_borrowed_ptr(globals.py(), builtins)
    })
}

fn soac_ext_module_state(module: &Bound<'_, PyAny>) -> PyResult<*mut SoacExtModuleState> {
    unsafe {
        let module_def = ffi::PyModule_GetDef(module.as_ptr());
        if module_def != soac_ext_module_def() && module_def != soac_strict_module_def() {
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
    "exec",
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

fn source_named_generator_globals_require_ordinary_dict(
    module_name: &str,
    specialization_mode: Option<SpecializationMode>,
    function_kind: &FunctionKind,
    display_name: &str,
    has_original_runtime_code: bool,
) -> bool {
    module_name != "soac.runtime"
        && specialization_mode == Some(SpecializationMode::Apply)
        && *function_kind == FunctionKind::Generator
        && display_name != "<genexpr>"
        && has_original_runtime_code
}

unsafe fn tuple_from_global_names<'py>(
    py: Python<'py>,
    global_names: &[String],
) -> PyResult<Bound<'py, PyTuple>> {
    let tuple = unsafe { ffi::PyTuple_New(global_names.len() as ffi::Py_ssize_t) };
    let tuple = unsafe { Bound::from_owned_ptr_or_err(py, tuple)? }.cast_into::<PyTuple>()?;
    for (index, name) in global_names.iter().enumerate() {
        let mut item = unsafe {
            ffi::PyUnicode_FromStringAndSize(
                name.as_ptr().cast::<c_char>(),
                name.len() as ffi::Py_ssize_t,
            )
        };
        if item.is_null() {
            return Err(PyErr::fetch(py));
        }
        unsafe { ffi::PyUnicode_InternInPlace(&mut item) };
        if unsafe { ffi::PyTuple_SetItem(tuple.as_ptr(), index as ffi::Py_ssize_t, item) } != 0 {
            return Err(PyErr::fetch(py));
        }
    }
    Ok(tuple)
}

pub struct SoacExtModule;

impl SoacExtModule {
    pub(crate) fn install_interpreter_state(
        module: &Bound<'_, PyAny>,
        implementation: crate::strict_interpreter::InterpreterModuleState,
    ) -> PyResult<()> {
        let state = soac_ext_module_state(module)?;
        if unsafe { (*state).initialized } {
            return Err(crate::strict_runtime_unavailable(
                module.py(),
                "strict native module state was initialized twice",
            ));
        }
        unsafe {
            (*state)
                .implementation
                .write(ModuleImplementation::Interpreter(implementation));
            (*state).initialized = true;
        }
        Ok(())
    }

    /// Borrow only for callback-free inspection or snapshotting. Callers must
    /// not keep this Rust borrow across Python callbacks or native m_clear.
    pub(crate) fn with_interpreter_state<R>(
        module: &Bound<'_, PyAny>,
        f: impl FnOnce(Option<&crate::strict_interpreter::InterpreterModuleState>) -> PyResult<R>,
    ) -> PyResult<R> {
        let state = soac_ext_module_state(module)?;
        if !unsafe { (*state).initialized } {
            return Err(crate::strict_runtime_unavailable(
                module.py(),
                "strict module state is terminal",
            ));
        }
        match unsafe { (*state).implementation.assume_init_ref() } {
            ModuleImplementation::Interpreter(state) => f(Some(state)),
            ModuleImplementation::Soac { .. } => f(None),
        }
    }

    pub fn new(
        py: Python<'_>,
        spec: &Bound<'_, PyAny>,
        compile_session: &Arc<crate::session::CompileSession>,
        mut lowered_module: BlockPyModule<BlockPyModuleShape>,
        mut module_info: ModuleInfo,
        original_code_by_function_id: crate::AuthenticatedCodeCatalog,
        source: &str,
        strict_module: Arc<crate::VerifiedStrictModule>,
    ) -> PyResult<Py<PyAny>> {
        ensure_module_dict_metadata_names(&mut lowered_module.global_names);
        module_info.indexed_module_keys = lowered_module.global_names.clone();
        let source_hash = module_info.hash;
        let module_name = spec
            .getattr("name")?
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err("expected a module spec with a string 'name'"))?;
        let package_name = spec
            .getattr("parent")?
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err("expected a module spec with a string 'parent'"))?;
        match &lowered_module.strict_source {
            Some(stamp)
                if stamp.matches_verified(strict_module.type_facts())
                    && original_code_by_function_id.matches_verified(&strict_module)
                    && strict_module.source() == source.as_bytes()
                    && stamp.module.module_name == module_name
                    && stamp.module.source_hash == source_hash
                    && strict_module.interpreter_id()
                        == unsafe {
                            ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get())
                        } => {}
            _ => {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "strict module IR/source/interpreter authentication mismatch",
                ));
            }
        }
        let module = new_strict_module(py, spec, &module_name, &package_name)?;
        // Native installation preserves this exact dictionary and reserves its
        // prefix. Ordinary source never constructs a SOAC module object.
        let strict_runtime = crate::StrictModuleRuntimeState::install(py, &module, &strict_module)?;
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
                Some(strict_module),
                Some(strict_runtime),
            )?
        };
        Ok(module.unbind())
    }

    /// Native module-definition identity survives m_clear. In particular a
    /// terminal strict module is still ours: it must fail its owned execution
    /// checks, never retry through an ordinary source loader.
    pub fn owns_module(module: &Bound<'_, PyAny>) -> PyResult<bool> {
        if unsafe { ffi::PyModule_Check(module.as_ptr()) } == 0 {
            return Err(PyTypeError::new_err(
                "module execution requires a native module",
            ));
        }
        let definition = unsafe { ffi::PyModule_GetDef(module.as_ptr()) };
        if definition.is_null() && !unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Err(PyErr::fetch(module.py()));
        }
        Ok(definition == soac_ext_module_def() || definition == soac_strict_module_def())
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
    use soac_core::profile::{COUNTER_DUMP_MAGIC, parse_counter_dump_records};
    use soac_instrument::{InstrumentationConfig, define_typed_module_counter_defs};
    use soac_ir_typed::lower_blockpy_module_to_typed;
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn original_code_matching_preserves_named_and_nested_generator_code() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let source = r#"
def values(limit):
    yield from range(limit)

def consume(limit):
    return tuple(values(limit))

def generated(limit):
    return (value for value in range(limit))
"#;
            let lowered = lower_python_to_blockpy_for_testing(source)
                .expect("source-backed generators should lower")
                .blockpy_module;
            let original_module_code = PyModule::import(py, "builtins")
                .and_then(|builtins| builtins.getattr("compile"))
                .and_then(|compile| compile.call1((source, "<source-generator-test>", "exec")))
                .expect("source-backed generators should compile");
            let originals =
                match_original_code_to_functions(py, original_module_code.as_any(), &lowered)
                    .expect("original generator code should match the lowered module");

            for qualname in [
                "values",
                "consume",
                "generated",
                "generated.<locals>.<genexpr>",
            ] {
                let function = lowered
                    .callable_defs
                    .iter()
                    .find(|function| function.names.qualname == qualname)
                    .unwrap_or_else(|| panic!("lowered module should contain {qualname}"));
                let code = originals
                    .get(&function.function_id)
                    .unwrap_or_else(|| panic!("source code should contain {qualname}"));
                let original_qualname = code
                    .bind(py)
                    .getattr("co_qualname")
                    .and_then(|value| value.extract::<String>())
                    .expect("original code should expose its qualified name");
                assert_eq!(original_qualname, qualname);
            }
        });
    }

    #[test]
    fn generator_expression_code_exposure_matches_unique_native_iterable_spans() {
        use pyo3::types::{PyBytes, PyDict};
        use soac_contracts::SourceRange;
        use soac_core::block_py::GeneratorExpressionCode;
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            // Native-position kernel only. These ordinary code objects never
            // create strict runtime admission; the loader separately proves the
            // exact privately compiled source tree and compiler-creation edge.
            for (source, expressions, kind) in [
                (
                    "def same_line(values):\n    return (value for value in values), (value + 1 for value in values)\n",
                    vec![
                        ("(value for value in values)", "values"),
                        ("(value + 1 for value in values)", "values"),
                    ],
                    FunctionKind::Generator,
                ),
                (
                    "def nested(values):\n    return ((value for value in row) for row in values)\n",
                    vec![
                        ("((value for value in row) for row in values)", "values"),
                        ("(value for value in row)", "row"),
                    ],
                    FunctionKind::Generator,
                ),
                (
                    "def multiline(values):\n    return (\n        value\n        for value in values\n    )\n",
                    vec![(
                        "(\n        value\n        for value in values\n    )",
                        "values",
                    )],
                    FunctionKind::Generator,
                ),
                (
                    "def captured(offset):\n    return (offset + value for value in range(2))\n",
                    vec![("(offset + value for value in range(2))", "range(2)")],
                    FunctionKind::Generator,
                ),
                (
                    "async def asynchronous(values):\n    return (value async for value in values)\n",
                    vec![("(value async for value in values)", "values")],
                    FunctionKind::AsyncGenerator,
                ),
                (
                    "def call_one(values):\n    return tuple(implicit_item for implicit_item in values)\n",
                    vec![("(implicit_item for implicit_item in values)", "values")],
                    FunctionKind::Generator,
                ),
                (
                    "def call_multiline(values):\n    return tuple(\n        filtered_item\n        for filtered_item in values\n        if filtered_item\n    )\n",
                    vec![(
                        "(\n        filtered_item\n        for filtered_item in values\n        if filtered_item\n    )",
                        "values",
                    )],
                    FunctionKind::Generator,
                ),
                (
                    "def call_parenthesized(values):\n    return tuple((explicit_item for explicit_item in values))\n",
                    vec![("(explicit_item for explicit_item in values)", "values")],
                    FunctionKind::Generator,
                ),
            ] {
                let root = PyModule::import(py, "builtins")
                    .unwrap()
                    .getattr("compile")
                    .unwrap()
                    .call1((source, "<genexpr-position-test>", "exec"))
                    .unwrap();
                let code_type = PyModule::import(py, "types")
                    .unwrap()
                    .getattr("CodeType")
                    .unwrap();
                let mut by_qualname = OriginalCodeByQualname::new();
                collect_original_code_objects(&root, &code_type, &mut by_qualname).unwrap();
                let codes = by_qualname
                    .values()
                    .flat_map(|codes| codes.iter().map(|code| code.bind(py).clone()))
                    .collect::<Vec<_>>();
                let line_starts = std::iter::once(0)
                    .chain(
                        source
                            .bytes()
                            .enumerate()
                            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
                    )
                    .collect::<Vec<_>>();
                let mut selected = HashSet::new();
                for (expression, iterable) in expressions {
                    let start = source.find(expression).unwrap();
                    let iterable_start = start + expression.rfind(iterable).unwrap();
                    let projection = GeneratorExpressionCode {
                        expression_range: SourceRange::new(
                            start as u32,
                            (start + expression.len()) as u32,
                        ),
                        iterable_range: SourceRange::new(
                            iterable_start as u32,
                            (iterable_start + iterable.len()) as u32,
                        ),
                    };
                    let index = match_native_generator_expression_code(
                        &codes,
                        &projection,
                        &kind,
                        &line_starts,
                    )
                    .unwrap()
                    .expect("exact parser-owned expression has its native code");
                    assert!(
                        selected.insert(index),
                        "same-line/nested source occurrences remain distinct"
                    );
                    let code = &codes[index];
                    for variant in 0..3 {
                        let mut changed = projection.clone();
                        match variant {
                            0 => changed.iterable_range.end -= 1,
                            1 => changed.expression_range.end = changed.iterable_range.end - 1,
                            _ => changed.expression_range.start = changed.expression_range.end,
                        }
                        assert!(
                            !strict_native_generator_expression_matches(
                                code,
                                &changed,
                                &kind,
                                &line_starts
                            )
                            .unwrap()
                        );
                    }
                    assert!(
                        !strict_native_generator_expression_matches(
                            code,
                            &projection,
                            &FunctionKind::Function,
                            &line_starts
                        )
                        .unwrap()
                    );
                    let duplicate = vec![code.clone(), code.clone()];
                    assert!(
                        match_native_generator_expression_code(
                            &duplicate,
                            &projection,
                            &kind,
                            &line_starts
                        )
                        .is_err(),
                        "an ambiguous native occurrence cannot be chosen by order"
                    );
                    let replacements = PyDict::new(py);
                    replacements
                        .set_item("co_linetable", PyBytes::new(py, b""))
                        .unwrap();
                    let without_positions = code
                        .call_method("replace", (), Some(&replacements))
                        .unwrap();
                    assert!(
                        !matches!(
                            strict_native_generator_expression_matches(
                                &without_positions,
                                &projection,
                                &kind,
                                &line_starts
                            ),
                            Ok(true)
                        ),
                        "a name/header without native positions is not a code-exposure witness"
                    );
                    let replacements = PyDict::new(py);
                    replacements.set_item("co_name", "other").unwrap();
                    let renamed = code
                        .call_method("replace", (), Some(&replacements))
                        .unwrap();
                    assert!(
                        !strict_native_generator_expression_matches(
                            &renamed,
                            &projection,
                            &kind,
                            &line_starts
                        )
                        .unwrap()
                    );
                }
            }
        });
    }

    #[test]
    fn strict_native_named_noop_bodies_keep_their_exact_header_anchor() {
        use pyo3::types::PyBytes;
        use soac_contracts::{
            AnnotationOrigin, CallableSignature, DefinitionKind, FunctionTypeFact, ModuleTypeFacts,
            ResolvedStrictPolicy, SourceDialect, SourceIdentity, SourceRange, StaticType,
        };

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            for (source, qualname, first_line, kind) in [
                ("def f():\n    pass\n", "f", 1, FunctionKind::Function),
                (
                    "def f():\n    global other\n",
                    "f",
                    1,
                    FunctionKind::Function,
                ),
                ("def f():\n    value: int\n", "f", 1, FunctionKind::Function),
                (
                    "def f():\n    'only documentation'\n",
                    "f",
                    1,
                    FunctionKind::Function,
                ),
                (
                    "@decorate\ndef f():\n    global other\n",
                    "f",
                    1,
                    FunctionKind::Function,
                ),
                (
                    "async def f():\n    global other\n",
                    "f",
                    1,
                    FunctionKind::Coroutine,
                ),
                (
                    "def owner():\n    value = 1\n    def f():\n        nonlocal value\n",
                    "owner.<locals>.f",
                    3,
                    FunctionKind::Function,
                ),
            ] {
                let lowered = lower_python_to_blockpy_for_testing(source)
                    .unwrap()
                    .blockpy_module;
                let function = lowered
                    .callable_defs
                    .iter()
                    .find(|function| {
                        function.names.qualname == qualname && *function.lowered_kind() == kind
                    })
                    .expect("the original source function is lowered");
                // This is the native-position kernel only: compile the exact
                // source, never execute decorators or fabricate admission.
                let original = PyModule::import(py, "builtins")
                    .unwrap()
                    .getattr("compile")
                    .unwrap()
                    .call1((source, "<named-noop-position-test>", "exec"))
                    .unwrap();
                let code_type = PyModule::import(py, "types")
                    .unwrap()
                    .getattr("CodeType")
                    .unwrap();
                let mut codes = OriginalCodeByQualname::new();
                collect_original_code_objects(&original, &code_type, &mut codes).unwrap();
                let candidates = &codes[qualname];
                assert_eq!(candidates.len(), 1, "one exact native named definition");
                let code = candidates.front().unwrap().bind(py);
                let line_starts = std::iter::once(0)
                    .chain(
                        source
                            .bytes()
                            .enumerate()
                            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
                    )
                    .collect::<Vec<_>>();
                let facts = ModuleTypeFacts::new(
                    "named_noop_position_test",
                    source.as_bytes(),
                    SourceDialect::OrdinaryPython,
                    ResolvedStrictPolicy::default(),
                )
                .unwrap();
                let definition_start = source
                    .find("async def f")
                    .or_else(|| source.find("def f"))
                    .unwrap();
                let mut fact = FunctionTypeFact {
                    identity: SourceIdentity {
                        module: facts.module,
                        lexical_qualname: qualname.to_owned(),
                        source_range: SourceRange::new(
                            definition_start as u32,
                            source.trim_end().len() as u32,
                        ),
                        definition_kind: DefinitionKind::Function,
                    },
                    function_kind: match kind {
                        FunctionKind::Coroutine => soac_contracts::FunctionKind::Coroutine,
                        _ => soac_contracts::FunctionKind::Synchronous,
                    },
                    signature: CallableSignature {
                        parameters: Vec::new(),
                        return_type: StaticType::Unknown,
                        return_annotation_origin: AnnotationOrigin::Absent,
                        uncertainty: Default::default(),
                    },
                    decorators: Vec::new(),
                    uncertainty: Default::default(),
                };
                assert!(
                    strict_native_code_matches(code, function, &fact, &line_starts, first_line)
                        .unwrap(),
                    "named no-op body should retain its native witness: {source}"
                );
                assert!(
                    !strict_native_code_matches(
                        code,
                        function,
                        &fact,
                        &line_starts,
                        first_line + 1,
                    )
                    .unwrap(),
                    "a header-only witness still requires its exact first line"
                );
                let replacements = PyDict::new(py);
                replacements
                    .set_item("co_linetable", PyBytes::new(py, b""))
                    .unwrap();
                let without_positions = code
                    .call_method("replace", (), Some(&replacements))
                    .unwrap();
                assert!(
                    without_positions
                        .getattr("co_linetable")
                        .unwrap()
                        .cast::<PyBytes>()
                        .unwrap()
                        .as_bytes()
                        .is_empty()
                );
                assert_eq!(
                    strict_native_location_code_length(&without_positions).unwrap(),
                    None
                );
                assert!(
                    !matches!(
                        strict_native_code_matches(
                            &without_positions,
                            function,
                            &fact,
                            &line_starts,
                            first_line,
                        ),
                        Ok(true)
                    ),
                    "entirely absent locations cannot become a named-body witness"
                );
                if source.contains("global other") {
                    fact.identity.definition_kind = DefinitionKind::Lambda;
                    assert!(
                        !strict_native_code_matches(
                            code,
                            function,
                            &fact,
                            &line_starts,
                            first_line,
                        )
                        .unwrap(),
                        "anonymous definitions still require a nonempty source span"
                    );
                }
            }
        });
    }

    #[test]
    fn strict_native_generic_wrappers_require_the_exact_header_span() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            for (name, body) in [
                ("Item", "class Item[T]:\n    value: T\n"),
                ("item", "def item[T](value: T) -> T:\n    return value\n"),
                (
                    "item",
                    "async def item[T](value: T) -> T:\n    return value\n",
                ),
            ] {
                let source = format!(
                    "from __future__ import strict\n@decorator\n# between decorator and header\n{body}"
                );
                // Compile only. The unresolved decorator is never called, and
                // this native-position kernel grants no runtime admission.
                let original = PyModule::import(py, "builtins")
                    .unwrap()
                    .getattr("compile")
                    .unwrap()
                    .call1((&source, "<generic-header-range-test>", "exec"))
                    .unwrap();
                let code_type = PyModule::import(py, "types")
                    .unwrap()
                    .getattr("CodeType")
                    .unwrap();
                let mut codes = OriginalCodeByQualname::new();
                collect_original_code_objects(&original, &code_type, &mut codes).unwrap();
                let candidates = &codes[&format!("<generic parameters of {name}>")];
                assert_eq!(candidates.len(), 1);
                let code = candidates[0].bind(py);
                assert_eq!(
                    code.getattr("co_firstlineno")
                        .unwrap()
                        .extract::<usize>()
                        .unwrap(),
                    2
                );
                let line_starts = std::iter::once(0)
                    .chain(
                        source
                            .bytes()
                            .enumerate()
                            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
                    )
                    .collect::<Vec<_>>();
                let native = soac_contracts::SourceRange::new(
                    source.find(body).unwrap() as u32,
                    source.trim_end().len() as u32,
                );
                assert!(
                    strict_native_type_expression_range_matches(code, &native, &line_starts)
                        .unwrap()
                );
                for wrong in [
                    soac_contracts::SourceRange::new(
                        source.find("@decorator").unwrap() as u32,
                        native.end,
                    ),
                    soac_contracts::SourceRange::new(native.start + 1, native.end),
                    soac_contracts::SourceRange::new(native.start, native.end - 1),
                    soac_contracts::SourceRange::new(native.start, native.end + 1),
                ] {
                    assert!(
                        !strict_native_type_expression_range_matches(code, &wrong, &line_starts)
                            .unwrap(),
                        "{body}: {wrong:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn apply_source_named_generator_globals_require_ordinary_dict() {
        assert!(source_named_generator_globals_require_ordinary_dict(
            "pkg.workload",
            Some(SpecializationMode::Apply),
            &FunctionKind::Generator,
            "items",
            true,
        ));
    }

    #[test]
    fn countered_and_unspecialized_generator_globals_remain_indexed() {
        for specialization_mode in [
            None,
            Some(SpecializationMode::Profile),
            Some(SpecializationMode::Verify),
        ] {
            assert!(!source_named_generator_globals_require_ordinary_dict(
                "pkg.workload",
                specialization_mode,
                &FunctionKind::Generator,
                "items",
                true,
            ));
        }
    }

    #[test]
    fn generator_expression_globals_remain_indexed() {
        assert!(!source_named_generator_globals_require_ordinary_dict(
            "pkg.workload",
            Some(SpecializationMode::Apply),
            &FunctionKind::Generator,
            "<genexpr>",
            true,
        ));
    }

    #[test]
    fn generated_and_ordinary_function_globals_remain_indexed() {
        assert!(!source_named_generator_globals_require_ordinary_dict(
            "pkg.workload",
            Some(SpecializationMode::Apply),
            &FunctionKind::Generator,
            "items",
            false,
        ));
        assert!(!source_named_generator_globals_require_ordinary_dict(
            "pkg.workload",
            Some(SpecializationMode::Apply),
            &FunctionKind::Function,
            "items",
            true,
        ));
    }

    #[test]
    fn compiler_runtime_generator_globals_remain_indexed() {
        assert!(!source_named_generator_globals_require_ordinary_dict(
            "soac.runtime",
            Some(SpecializationMode::Apply),
            &FunctionKind::Generator,
            "code_template_gen",
            true,
        ));
    }

    fn define_module_block_entry_counters(module: &mut BlockPyModule<BlockPyModuleShape>) {
        let config = InstrumentationConfig::from_env_config(
            &SoacEnvConfig::default()
                .with_specialization_mode(Some(SpecializationMode::Profile))
                .with_profiled_cold_blocks_enabled(true),
        );
        let mut typed_for_counters = lower_blockpy_module_to_typed(module.clone());
        define_typed_module_counter_defs(&mut typed_for_counters, &config)
            .expect("typed block-entry counter definitions should succeed");
        module.counter_defs = typed_for_counters.counter_defs;
    }

    #[test]
    fn counter_dump_record_includes_block_entry_metadata_and_value() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
def f():
    return None
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        define_module_block_entry_counters(&mut lowered);

        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .expect("missing lowered function f");
        let function_id = function.function_id;
        let entry_label = function.entry_block().label;
        let entry_label_text = entry_label.to_string();

        let shared_state = SharedModuleState {
            strict_module: None,
            strict_execution: None,
            late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
                &lowered,
                "counter_test",
            ),
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            source_hash: 0,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            function_templates: Mutex::new(HashMap::new()),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0)].into_boxed_slice(),
            counter_values: vec![3].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: String::new(),
            original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
                HashMap::new(),
            ),
            deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
            counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
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
        .blockpy_module;

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
            branches: Vec::new(),
        });

        let shared_state = SharedModuleState {
            strict_module: None,
            strict_execution: None,
            late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
                &lowered,
                "counter_test",
            ),
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            source_hash: 0,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            function_templates: Mutex::new(HashMap::new()),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0)].into_boxed_slice(),
            counter_values: vec![5].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: String::new(),
            original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
                HashMap::new(),
            ),
            deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
            counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
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

        // Two compiled tables for the same source function keep independent
        // ordinals/IDs. Registering either must not move live scalar storage,
        // and retiring a table must not discard its already-observed events.
        let scalar_storage = shared_state.scalar_counter_values_ptr();
        let first = shared_state
            .register_deopt_entry_counters(
                function_id,
                vec![
                    DeoptEntrySource::BeforeTerm {
                        block_label: entry_label,
                    },
                    DeoptEntrySource::BlockEntry {
                        block_label: entry_label,
                    },
                ],
            )
            .expect("first compiled table counters should register");
        let second = shared_state
            .register_deopt_entry_counters(
                function_id,
                vec![DeoptEntrySource::BeforeTerm {
                    block_label: entry_label,
                }],
            )
            .expect("second compiled table counters should register");
        first.record(0);
        first.record(0);
        second.record(0);
        drop(first);
        assert_eq!(shared_state.scalar_counter_values_ptr(), scalar_storage);
        assert_eq!(shared_state.counter_values.len(), 1);
        let rows = shared_state.counter_dump_record().unwrap().rows;
        assert_eq!(
            rows.iter()
                .map(|row| (row.counter_id, row.value))
                .collect::<Vec<_>>(),
            vec![(0, 5), (1, 2), (2, 0), (3, 1)]
        );
        assert!(rows.iter().all(|row| {
            row.kind == "deopt_entry_guard_miss"
                && row.site_kind == "deopt_entry"
                && row.function_id == Some(function_id)
                && row.function_qualname.as_deref() == Some("f")
                && row.block_label.as_deref() == Some(entry_label_text.as_str())
        }));
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
                branches: Vec::new(),
            },
            CounterDef {
                id: CounterId(1),
                scope: CounterScope::Function,
                kind: "runtime_incref".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(RuntimeFunctionId::from_raw_parts(0, 7)),
                    instr_id: None,
                },
                branches: Vec::new(),
            },
            CounterDef {
                id: CounterId(2),
                scope: CounterScope::Global,
                kind: "runtime_decref".to_string(),
                site: CounterSite::Runtime {
                    function_id: None,
                    instr_id: None,
                },
                branches: Vec::new(),
            },
            CounterDef {
                id: CounterId(3),
                scope: CounterScope::Global,
                kind: "runtime_decref".to_string(),
                site: CounterSite::Runtime {
                    function_id: None,
                    instr_id: None,
                },
                branches: Vec::new(),
            },
            CounterDef {
                id: CounterId(4),
                scope: CounterScope::This,
                kind: "block_entry".to_string(),
                site: CounterSite::BlockEntry {
                    function_id: RuntimeFunctionId::from_raw_parts(0, 7),
                    block_label: soac_core::block_py::BlockLabel::from_index(0),
                },
                branches: Vec::new(),
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
    fn branch_counter_storage_uses_one_counter_id_with_named_slots() {
        let counter_defs = vec![CounterDef {
            id: CounterId(0),
            scope: CounterScope::This,
            kind: "call_direct".to_string(),
            site: CounterSite::Runtime {
                function_id: Some(RuntimeFunctionId::from_raw_parts(0, 7)),
                instr_id: None,
            },
            branches: vec![
                soac_core::block_py::CounterBranch::new("hit"),
                soac_core::block_py::CounterBranch::new("fallback"),
            ],
        }];

        let (slots_by_id, counter_values, top_value_counters) =
            build_counter_storage(&counter_defs).expect("counter storage should build");

        assert_eq!(counter_values.len(), 2);
        assert!(top_value_counters.is_empty());
        assert_eq!(
            slots_by_id[0],
            CounterRuntimeSlot::Branches { start: 0, len: 2 }
        );
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
            let (layouts, type_table) =
                snapshot_type_key_layout_events_bound(events.as_any(), "type_event_test")
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
    fn watched_preseeded_split_keys_are_present_in_profile_snapshot() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "watched_preseeded_split_keys_are_present_in_profile_snapshot",
        ) {
            return;
        }

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe { std::env::set_var("SOAC_OPT_MODE", "profile") };
        crate::initialize_test_python();

        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"
class MetadataBlockingMeta(type):
    def __getattribute__(cls, name):
        if name in ('__module__', '__qualname__'):
            raise AssertionError('profiling must not call metaclass attribute hooks')
        return type.__getattribute__(cls, name)

class Point(metaclass=MetadataBlockingMeta):
    def populate(self):
        self.zeta = 1
        self.alpha = 2

class Uninstantiated:
    def populate(self):
        self.abstract_only = 1
",
                c"preseeded_type_events.py",
                c"preseeded_type_event_test",
            )
            .expect("test module should execute");
            let owner = module.getattr("Point").expect("owner should exist");
            let uninstantiated = module
                .getattr("Uninstantiated")
                .expect("uninstantiated owner should exist");
            unsafe { watch_split_keys_for_type(owner.as_ptr()) }
                .expect("production watcher should accept a heap type");
            unsafe { watch_split_keys_for_type(uninstantiated.as_ptr()) }
                .expect("production watcher should accept an uninstantiated heap type");
            owner.call0().expect("owner should instantiate");

            let (layouts, type_table) =
                snapshot_type_key_layout_events("preseeded_type_event_test");
            let owner_entry = type_table
                .iter()
                .find(|entry| {
                    entry.key.module_name == "preseeded_type_event_test"
                        && entry.key.qualname == "Point"
                })
                .expect("profile snapshot must retain the preseeded owner type");
            let owner_layouts = layouts
                .into_iter()
                .filter(|layout| layout.owner_type_id == owner_entry.type_id)
                .map(|layout| (layout.key, layout.index))
                .collect::<Vec<_>>();

            assert_eq!(
                owner_layouts,
                [("alpha".to_string(), 0), ("zeta".to_string(), 1)]
            );
            assert!(
                !type_table
                    .iter()
                    .any(|entry| entry.key.qualname == "Uninstantiated"),
                "uninstantiated classes must not manufacture exact-owner profile evidence"
            );
        });
    }

    #[test]
    fn profile_type_ids_are_stable_across_registries() {
        let type_key = CounterDumpTypeKey {
            module_name: "__main__".to_string(),
            qualname: "benchmark_reduce.<locals>.C".to_string(),
        };
        let mut first_registry = ProfileTypeRegistry::default();
        let mut second_registry = ProfileTypeRegistry::default();

        let first_id = first_registry
            .id_for_type(0x1000, type_key.clone())
            .expect("first registry should assign an id");
        let second_id = second_registry
            .id_for_type(0x2000, type_key)
            .expect("second registry should assign the same id");

        assert_eq!(first_id, second_id);
        assert_ne!(first_id, 0);
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
        .blockpy_module;
        define_module_block_entry_counters(&mut lowered);

        let shared_state = SharedModuleState {
            strict_module: None,
            strict_execution: None,
            late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
                &lowered,
                "counter_test",
            ),
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            source_hash: 0,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            function_templates: Mutex::new(HashMap::new()),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0), CounterRuntimeSlot::Scalar(1)]
                .into_boxed_slice(),
            counter_values: vec![5, 8].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_test".to_string(),
            package_name: "pkg".to_string(),
            original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
                HashMap::new(),
            ),
            deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
            counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
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

    #[test]
    fn counter_dump_flush_is_path_aware_reentrant_and_retries_failed_writes() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
VALUE = 1

def f():
    return VALUE
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        define_module_block_entry_counters(&mut lowered);

        let shared_state = SharedModuleState {
            strict_module: None,
            strict_execution: None,
            late_bound_owner_fields: LateBoundOwnerFieldRuntime::for_module(
                &lowered,
                "counter_flush_test",
            ),
            function_index_by_id: build_function_index_by_id(&lowered)
                .expect("function index should build"),
            codegen_constants: ModuleCodegenConstants::collect_from_module(&lowered),
            source_hash: 0,
            storage_instance_key: allocate_shared_module_state_storage_key(),
            function_templates: Mutex::new(HashMap::new()),
            module_constant_objs: Vec::new(),
            runtime_name_cache: build_runtime_name_cache(),
            counter_slots_by_id: vec![CounterRuntimeSlot::Scalar(0), CounterRuntimeSlot::Scalar(1)]
                .into_boxed_slice(),
            counter_values: vec![5, 8].into_boxed_slice(),
            top_value_counters: Vec::new().into_boxed_slice(),
            lowered_module: lowered,
            module_name: "counter_flush_test".to_string(),
            package_name: "pkg".to_string(),
            original_code_by_function_id: crate::strict_admission::OriginalCodeStorage::Inspection(
                HashMap::new(),
            ),
            deopt_entry_counters: Mutex::new(DeoptEntryCounterRegistry::default()),
            counter_dump_flush_tracker: Mutex::new(CounterDumpFlushTracker::default()),
        };

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "soac_counter_dump_flush_{unique}_{}",
            std::process::id()
        ));
        let profile_path = dir.join("profile.bin");
        assert!(
            shared_state
                .flush_counter_dump_file_once(profile_path.as_path())
                .is_err(),
            "a missing output directory should leave the flush eligible for retry",
        );

        fs::create_dir_all(&dir).expect("counter dump output directory should be creatable");
        shared_state
            .flush_counter_dump_file_once(profile_path.as_path())
            .expect("failed counter dump write should be retryable");
        shared_state
            .flush_counter_dump_file_once(profile_path.as_path())
            .expect("completed profile dump should not be appended twice");
        let profile = fs::read(&profile_path).expect("profile dump should be readable");
        assert_eq!(
            parse_counter_dump_records(profile.as_slice())
                .expect("profile dump should decode")
                .len(),
            1,
            "a completed output path must contain exactly one module record",
        );

        let verify_path = dir.join("verify.bin");
        shared_state
            .flush_counter_dump_file_once(verify_path.as_path())
            .expect("a distinct verification path should receive its own record");
        let verify = fs::read(&verify_path).expect("verification dump should be readable");
        assert_eq!(
            parse_counter_dump_records(verify.as_slice())
                .expect("verification dump should decode")
                .len(),
            1,
        );

        let reentrant_path = dir.join("reentrant.bin");
        shared_state
            .counter_dump_flush_tracker
            .lock()
            .expect("flush tracker should lock")
            .paths
            .insert(reentrant_path.clone(), CounterDumpFlushStatus::InProgress);
        shared_state
            .flush_counter_dump_file_once(reentrant_path.as_path())
            .expect("a reentrant flush of the same path should be skipped");
        assert!(!reentrant_path.exists());

        shared_state
            .mark_counter_dump_module_cleared()
            .expect("cleared module marker should be writable");
        let stale_path = dir.join("stale.bin");
        shared_state
            .flush_counter_dump_file_once(stale_path.as_path())
            .expect("a cleared module should not enter a later output path");
        assert!(!stale_path.exists());

        fs::remove_dir_all(&dir).expect("counter dump test directory should be removable");
    }
}
