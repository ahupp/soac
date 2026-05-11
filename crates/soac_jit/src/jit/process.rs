use super::backend::{
    CompiledFunctionArtifact, compile_prepared_function_bytes,
    compile_prepared_function_bytes_with_purpose_aliases, define_compiled_function_bytes,
    new_jit_module, record_jit_bb_map, register_jit_signal_diagnostics,
};
use super::clif_function_display_aliases;
use super::codegen_env::JitCodegenEnv;
use super::compiled::{
    CompiledFunctionHandle, DirectFunctionCompileResult, JitCodegenStats, VectorcallEntryFn,
};
use super::deopt::RuntimeJitDeoptTable;
use super::direct_function::{build_default_resolving_direct_adapter, declare_direct_function};
use super::function_targets::{
    collect_call_direct_targets, collect_make_function_targets,
    collect_planned_typed_call_direct_targets, is_synthetic_class_helper_function,
};
use super::imports::{predeclare_jit_runtime_imports, predeclare_specialization_type_imports};
use super::jitdump;
use super::module_data::{
    declare_module_constant_object_data_for_prefix, declare_scalar_counter_storage_import,
    declare_top_value_counter_storage_import, define_scalar_counter_storage_data_for_symbol,
    define_top_value_counter_storage_data_for_symbol,
    direct_function_symbol_scope_for_shared_state, module_constant_symbol_prefix_for_instance,
    module_constant_symbol_prefix_for_shared_state, scalar_counter_storage_symbol_for_instance,
    scalar_counter_storage_symbol_for_shared_state, top_value_counter_storage_symbol_for_instance,
    top_value_counter_storage_symbol_for_shared_state,
};
use super::specialized_helpers::ObjPtr;
use super::symbols::{
    direct_function_backend_name, direct_function_symbol_scope, register_jit_data_symbol,
};
use super::typed_pipeline::{
    JitModulePlan, collect_codegen_constants_for_module_name, optimize_blockpy,
    optimize_blockpy_for_shared_state,
};
use super::vectorcall::define_shared_vectorcall_trampoline;
use super::{
    BuildSpecializedFunctionOptions, CountedRefcountHelpers, DeclaredJitFunction,
    PROCESS_JIT_COMPILE_DEPTH, PlannedOptimizationInputs, SpecializationProfile,
    build_counted_runtime_refcount_helpers, build_cranelift_run_bb_specialized_function,
    load_planned_optimization_inputs_for_runtime_state,
};
use crate::config::CraneliftTargetConfig;
use crate::module_constants::ModuleCodegenConstants;
use crate::module_type::{CounterRuntimeSlot, build_counter_storage_layout};
use cranelift_codegen::ir;
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId, Linkage, Module};
use pyo3::{Py, PyAny, Python, ffi};
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, CounterDef, FunctionExecutionMode, InstrId, RuntimeFunctionId,
};
use soac_ir_blockpy::BlockPyModuleShape;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tracing::{info, warn};

struct CompiledJitFunction {
    function_id: RuntimeFunctionId,
    function_qualname: String,
    param_count: usize,
    main_id: FuncId,
    main_symbol: String,
    default_adapter_id: Option<FuncId>,
    default_adapter_symbol: Option<String>,
    stats: JitCodegenStats,
    compiled: CompiledFunctionArtifact,
    default_adapter_compiled: Option<CompiledFunctionArtifact>,
    deopt_table: Arc<RuntimeJitDeoptTable>,
}

struct DirectFunctionCompileInputs<'a> {
    session: &'a Arc<crate::session::CompileSession>,
    blocks: &'a [ObjPtr],
    module: &'a BlockPyModule<BlockPyModuleShape>,
    module_constants: &'a ModuleCodegenConstants,
    counter_defs: &'a [CounterDef],
    module_constant_ptrs: &'a [*mut ffi::PyObject],
    direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
}

// Worker codegen treats these as immutable inputs. The raw Python pointers are copied into
// generated metadata or used as opaque identity values; Python-visible lookup/materialization work
// must happen during the serial reservation/commit phases.
unsafe impl<'a> Send for DirectFunctionCompileInputs<'a> {}
unsafe impl<'a> Sync for DirectFunctionCompileInputs<'a> {}

struct ReservedJitFunctionCompileInputs {
    module_constant_ptrs: Vec<*mut ffi::PyObject>,
    module_constant_owners: Option<Arc<Vec<Py<PyAny>>>>,
    counter_slots_by_id: Vec<CounterRuntimeSlot>,
    module_constant_object_data_ids: Vec<DataId>,
    scalar_counter_data_id: Option<DataId>,
    top_value_counter_data_id: Option<DataId>,
    counted_refcount_helpers: CountedRefcountHelpers,
    module_constant_binding_key: usize,
    symbol_scope: Option<String>,
}

// Reservation records are frozen before worker codegen starts. Workers only read ids, counter slot
// offsets, and raw module-constant pointer values that were already installed in the process JIT
// namespace during the serial phase.
unsafe impl Send for ReservedJitFunctionCompileInputs {}
unsafe impl Sync for ReservedJitFunctionCompileInputs {}

#[derive(Clone)]
struct JitModuleDeclarationSnapshot {
    functions: Vec<JitFunctionDeclarationSnapshot>,
    data_objects: Vec<JitDataDeclarationSnapshot>,
}

#[derive(Clone)]
struct JitFunctionDeclarationSnapshot {
    id: FuncId,
    name: Option<String>,
    linkage: Linkage,
    signature: ir::Signature,
}

#[derive(Clone)]
struct JitDataDeclarationSnapshot {
    id: DataId,
    name: Option<String>,
    linkage: Linkage,
    writable: bool,
    tls: bool,
}

struct JitBatchPlan<'a> {
    root_function_id: RuntimeFunctionId,
    env_config: SoacEnvConfig,
    batch_functions: Vec<ProcessJitBatchFunction<'a>>,
    function_indices_to_define: Vec<usize>,
    function_compile_inputs: HashMap<usize, ReservedJitFunctionCompileInputs>,
    compile_waiters: HashMap<RuntimeFunctionId, Arc<ProcessJitCompileWaiter>>,
    module_declarations: JitModuleDeclarationSnapshot,
    isa: OwnedTargetIsa,
    module_plans: HashMap<usize, Arc<JitModulePlan>>,
    predeclared: HashMap<RuntimeFunctionId, DeclaredJitFunction>,
}

enum ReservedDirectFunctionBatch<'a> {
    Ready(Arc<CompiledFunctionHandle>),
    Compiling(Arc<ProcessJitCompileWaiter>),
    Reserved(JitBatchPlan<'a>),
}

struct JitBatchWorkQueue {
    inner: Mutex<JitBatchWorkQueueInner>,
}

struct JitBatchWork {
    queue: JitBatchWorkQueue,
    assist_context: Option<Arc<JitBatchAssistContext>>,
}

struct JitBatchAssistContext {
    session: Arc<crate::session::CompileSession>,
    plan: Arc<JitBatchPlan<'static>>,
    shared_state: Arc<crate::module_type::SharedModuleState>,
    blocks: Vec<ObjPtr>,
    module_constant_ptrs: Vec<*mut ffi::PyObject>,
    dependencies: HashMap<RuntimeFunctionId, HashSet<RuntimeFunctionId>>,
    index_by_function_id: HashMap<RuntimeFunctionId, usize>,
}

// Foreground assists run on the Python thread while background workers are compiling from the same
// immutable plan. The raw pointers are the same opaque Python object/block identities already used
// by DirectFunctionCompileInputs, whose Send/Sync safety is documented above.
unsafe impl Send for JitBatchAssistContext {}
unsafe impl Sync for JitBatchAssistContext {}

struct JitBatchWorkQueueInner {
    queued: VecDeque<usize>,
    index_by_function_id: HashMap<RuntimeFunctionId, usize>,
}

impl JitBatchWorkQueue {
    fn new(plan: &JitBatchPlan<'_>) -> Self {
        let mut index_by_function_id =
            HashMap::with_capacity(plan.function_indices_to_define.len());
        for batch_function_index in &plan.function_indices_to_define {
            let function_id = plan.batch_functions[*batch_function_index]
                .function
                .function_id;
            index_by_function_id.insert(function_id, *batch_function_index);
        }
        Self {
            inner: Mutex::new(JitBatchWorkQueueInner {
                queued: VecDeque::from(plan.function_indices_to_define.clone()),
                index_by_function_id,
            }),
        }
    }

    fn pop_front(&self) -> Result<Option<usize>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "process JIT background work queue lock poisoned".to_string())?;
        Ok(inner.queued.pop_front())
    }

    fn promote_function(&self, function_id: RuntimeFunctionId) -> Result<bool, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "process JIT background work queue lock poisoned".to_string())?;
        let Some(batch_function_index) = inner.index_by_function_id.get(&function_id).copied()
        else {
            return Ok(false);
        };
        let Some(position) = inner
            .queued
            .iter()
            .position(|queued_index| *queued_index == batch_function_index)
        else {
            return Ok(false);
        };
        if position != 0 {
            inner.queued.remove(position);
            inner.queued.push_front(batch_function_index);
        }
        Ok(true)
    }

    fn take_function_indices(&self, requested_indices: &[usize]) -> Result<Vec<usize>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "process JIT background work queue lock poisoned".to_string())?;
        let mut claimed = Vec::new();
        for requested_index in requested_indices {
            if let Some(position) = inner
                .queued
                .iter()
                .position(|queued_index| queued_index == requested_index)
            {
                if let Some(claimed_index) = inner.queued.remove(position) {
                    claimed.push(claimed_index);
                }
            }
        }
        Ok(claimed)
    }
}

impl JitBatchWork {
    fn new(plan: &JitBatchPlan<'_>, assist_context: Option<Arc<JitBatchAssistContext>>) -> Self {
        Self {
            queue: JitBatchWorkQueue::new(plan),
            assist_context,
        }
    }

    fn pop_front(&self) -> Result<Option<usize>, String> {
        self.queue.pop_front()
    }

    fn promote_function(&self, function_id: RuntimeFunctionId) -> Result<bool, String> {
        self.queue.promote_function(function_id)
    }

    fn take_function_indices(&self, requested_indices: &[usize]) -> Result<Vec<usize>, String> {
        self.queue.take_function_indices(requested_indices)
    }

    fn assist_context(&self) -> Option<Arc<JitBatchAssistContext>> {
        self.assist_context.as_ref().map(Arc::clone)
    }
}

impl JitBatchAssistContext {
    fn new(
        session: Arc<crate::session::CompileSession>,
        plan: Arc<JitBatchPlan<'static>>,
        shared_state: Arc<crate::module_type::SharedModuleState>,
        blocks: Vec<ObjPtr>,
        module_constant_ptrs: Vec<*mut ffi::PyObject>,
        dependencies: HashMap<RuntimeFunctionId, HashSet<RuntimeFunctionId>>,
    ) -> Self {
        let mut index_by_function_id =
            HashMap::with_capacity(plan.function_indices_to_define.len());
        for batch_function_index in &plan.function_indices_to_define {
            let function_id = plan.batch_functions[*batch_function_index]
                .function
                .function_id;
            index_by_function_id.insert(function_id, *batch_function_index);
        }
        Self {
            session,
            plan,
            shared_state,
            blocks,
            module_constant_ptrs,
            dependencies,
            index_by_function_id,
        }
    }

    fn function_id_for_index(&self, batch_function_index: usize) -> RuntimeFunctionId {
        self.plan.batch_functions[batch_function_index]
            .function
            .function_id
    }

    fn dependency_closure_order(&self, function_id: RuntimeFunctionId) -> Vec<RuntimeFunctionId> {
        fn visit(
            dependencies: &HashMap<RuntimeFunctionId, HashSet<RuntimeFunctionId>>,
            function_id: RuntimeFunctionId,
            seen: &mut HashSet<RuntimeFunctionId>,
            out: &mut Vec<RuntimeFunctionId>,
        ) {
            if !seen.insert(function_id) {
                return;
            }
            if let Some(deps) = dependencies.get(&function_id) {
                for dep in deps {
                    visit(dependencies, *dep, seen, out);
                }
            }
            out.push(function_id);
        }

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        visit(&self.dependencies, function_id, &mut seen, &mut out);
        out
    }

    fn dependency_closure_indices(&self, function_id: RuntimeFunctionId) -> Vec<usize> {
        self.dependency_closure_order(function_id)
            .into_iter()
            .filter_map(|function_id| self.index_by_function_id.get(&function_id).copied())
            .collect()
    }

    fn compile_indices(
        &self,
        batch_function_indices: &[usize],
    ) -> Result<Vec<CompiledJitFunction>, String> {
        let inputs = DirectFunctionCompileInputs {
            session: &self.session,
            blocks: self.blocks.as_slice(),
            module: &self.shared_state.lowered_module,
            module_constants: &self.shared_state.codegen_constants,
            counter_defs: &self.shared_state.lowered_module.counter_defs,
            module_constant_ptrs: self.module_constant_ptrs.as_slice(),
            direct_call_resolver: Some(self.shared_state.as_ref()),
        };
        let mut codegen_env = ReservedJitCodegenEnv {
            isa: Arc::clone(&self.plan.isa),
            declarations: &self.plan.module_declarations,
        };
        ProcessJitState::compile_reserved_direct_function_batch_indices(
            &mut codegen_env,
            &inputs,
            self.plan.as_ref(),
            batch_function_indices,
        )
    }
}

struct JitBatchCompileOutput {
    compiled_functions: Vec<CompiledJitFunction>,
    worker_metrics: JitBatchWorkerMetrics,
}

struct JitBatchStreamingCommitOutput {
    committed_function_count: usize,
    commit_elapsed: Duration,
    worker_metrics: JitBatchWorkerMetrics,
}

const DEFAULT_JIT_COMPILE_WORKER_LIMIT: usize = 4;

#[derive(Clone, Copy, Debug, Default)]
struct JitBatchWorkerMetrics {
    requested_worker_count: usize,
    actual_worker_count: usize,
    worker_function_count_min: usize,
    worker_function_count_max: usize,
    worker_total_sum: Duration,
    worker_total_max: Duration,
    worker_setup_sum: Duration,
    worker_setup_max: Duration,
    worker_compile_sum: Duration,
    worker_compile_max: Duration,
}

#[derive(Clone, Copy, Debug)]
struct JitBatchWorkerTiming {
    function_count: usize,
    total: Duration,
    setup: Duration,
    compile: Duration,
}

struct JitBatchWorkerOutput {
    compiled_functions: Vec<CompiledJitFunction>,
    timing: JitBatchWorkerTiming,
}

enum JitBatchWorkerMessage {
    Compiled(Result<CompiledJitFunction, String>),
    Done(JitBatchWorkerTiming),
}

impl JitBatchWorkerMetrics {
    fn new(requested_worker_count: usize) -> Self {
        Self {
            requested_worker_count,
            worker_function_count_min: usize::MAX,
            ..Self::default()
        }
    }

    fn record_worker(&mut self, timing: JitBatchWorkerTiming) {
        self.actual_worker_count += 1;
        self.worker_function_count_min = self.worker_function_count_min.min(timing.function_count);
        self.worker_function_count_max = self.worker_function_count_max.max(timing.function_count);
        self.worker_total_sum += timing.total;
        self.worker_total_max = self.worker_total_max.max(timing.total);
        self.worker_setup_sum += timing.setup;
        self.worker_setup_max = self.worker_setup_max.max(timing.setup);
        self.worker_compile_sum += timing.compile;
        self.worker_compile_max = self.worker_compile_max.max(timing.compile);
    }

    fn finish(mut self) -> Self {
        if self.actual_worker_count == 0 {
            self.worker_function_count_min = 0;
        }
        self
    }
}

fn jit_batch_worker_count(function_count: usize, env_config: &SoacEnvConfig) -> usize {
    if function_count == 0 {
        return 0;
    }
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    let configured_limit = env_config
        .jit_compile_workers()
        .unwrap_or(DEFAULT_JIT_COMPILE_WORKER_LIMIT);
    function_count.min(available).min(configured_limit).max(1)
}

impl JitBatchPlan<'_> {
    fn precompute_module_plans(
        &mut self,
        inputs: &DirectFunctionCompileInputs<'_>,
    ) -> Result<(), String> {
        let env_config = inputs.session.env_config()?;
        for batch_function_index in &self.function_indices_to_define {
            let batch_function = &self.batch_functions[*batch_function_index];
            let reserved_inputs = self
                .function_compile_inputs
                .get(batch_function_index)
                .expect("reserved JIT batch function should have compile inputs");
            let binding_key = reserved_inputs.module_constant_binding_key;
            if self.module_plans.contains_key(&binding_key) {
                continue;
            }
            let module_plan = if let Some(shared_state) = batch_function.source.shared_state() {
                let profile = SpecializationProfile::from_runtime_state_with_session(
                    Some(shared_state),
                    Some(inputs.session.as_ref()),
                )?;
                optimize_blockpy_for_shared_state(
                    shared_state,
                    Some(inputs.session.as_ref()),
                    Some(&profile),
                    env_config,
                )?
            } else {
                optimize_blockpy(inputs.module, None, env_config)?
            };
            self.module_plans.insert(binding_key, module_plan);
        }
        Ok(())
    }

    fn prepare_planned_module_constant_bindings(
        &self,
    ) -> Result<Vec<PreparedModuleConstantRefresh>, String> {
        let mut prepared = Vec::new();
        let mut refreshed = HashSet::new();
        for batch_function_index in &self.function_indices_to_define {
            let batch_function = &self.batch_functions[*batch_function_index];
            let Some(shared_state) = batch_function.source.shared_state() else {
                continue;
            };
            let binding_key = self
                .function_compile_inputs
                .get(batch_function_index)
                .expect("reserved JIT batch function should have compile inputs")
                .module_constant_binding_key;
            if !refreshed.insert(binding_key) {
                continue;
            }
            let module_plan = self
                .module_plans
                .get(&binding_key)
                .expect("JIT module plan should be precomputed before refreshing constants");
            let planned_module = module_plan.module.as_ref();
            let codegen_constants = collect_codegen_constants_for_module_name(
                shared_state.module_name.as_str(),
                planned_module,
            );
            let owners = Python::attach(|py| {
                crate::module_type::build_module_constant_objects(
                    py,
                    &codegen_constants,
                    shared_state.module_name.as_str(),
                    shared_state.source_hash(),
                )
                .map_err(|err| err.to_string())
            })?;
            let ptrs = owners.iter().map(|obj| obj.as_ptr()).collect::<Vec<_>>();
            prepared.push(PreparedModuleConstantRefresh {
                binding_key,
                ptrs,
                owners: Arc::new(owners),
                planned_module_id: planned_module.module_name_gen.module_id(),
            });
        }
        Ok(prepared)
    }

    fn bind_prepared_module_constant_bindings(
        &mut self,
        prepared: Vec<PreparedModuleConstantRefresh>,
        state: &mut ProcessJitState,
        jit_module: &mut JITModule,
    ) -> Result<(), String> {
        let mut refreshed = HashMap::new();
        for refresh in prepared {
            let (object_binding_key, object_binding_id) =
                state.next_planned_module_constant_binding();
            let symbol_prefix = format!(
                "__soac_module_constant_{}_planned_{}",
                refresh.planned_module_id, object_binding_id
            );
            let data_ids = state.ensure_module_constant_objects(
                jit_module,
                refresh.ptrs.as_slice(),
                object_binding_key,
                symbol_prefix.as_str(),
            )?;
            refreshed.insert(
                refresh.binding_key,
                (refresh.ptrs, refresh.owners, data_ids),
            );
        }

        for reserved_inputs in self.function_compile_inputs.values_mut() {
            let Some((ptrs, owners, data_ids)) =
                refreshed.get(&reserved_inputs.module_constant_binding_key)
            else {
                continue;
            };
            reserved_inputs.module_constant_ptrs = ptrs.clone();
            reserved_inputs.module_constant_owners = Some(Arc::clone(owners));
            reserved_inputs.module_constant_object_data_ids = data_ids.clone();
        }
        Ok(())
    }
}

struct PreparedModuleConstantRefresh {
    binding_key: usize,
    ptrs: Vec<*mut ffi::PyObject>,
    owners: Arc<Vec<Py<PyAny>>>,
    planned_module_id: u32,
}

struct ReservedJitCodegenEnv<'a> {
    isa: OwnedTargetIsa,
    declarations: &'a JitModuleDeclarationSnapshot,
}

impl JitCodegenEnv for ReservedJitCodegenEnv<'_> {
    fn codegen_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
    }

    fn function_declaration(&self, id: FuncId) -> Result<(&ir::Signature, Linkage), String> {
        let declaration = self
            .declarations
            .function(id)
            .ok_or_else(|| format!("reserved JIT declaration snapshot is missing function {id}"))?;
        Ok((&declaration.signature, declaration.linkage))
    }

    fn data_declaration(&self, id: DataId) -> Result<(Linkage, bool), String> {
        let declaration = self
            .declarations
            .data(id)
            .ok_or_else(|| format!("reserved JIT declaration snapshot is missing data {id}"))?;
        Ok((declaration.linkage, declaration.tls))
    }

    fn codegen_declare_function(
        &mut self,
        name: &str,
        _linkage: Linkage,
        signature: &ir::Signature,
    ) -> Result<FuncId, String> {
        let declaration = self.declarations.function_by_name(name).ok_or_else(|| {
            format!("reserved JIT declaration snapshot is missing function symbol {name}")
        })?;
        if declaration.signature != *signature {
            return Err(format!(
                "reserved JIT declaration snapshot function {name} signature mismatch"
            ));
        }
        Ok(declaration.id)
    }

    fn codegen_declare_data(
        &mut self,
        name: &str,
        _linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> Result<DataId, String> {
        let declaration = self.declarations.data_by_name(name).ok_or_else(|| {
            format!("reserved JIT declaration snapshot is missing data symbol {name}")
        })?;
        if declaration.writable != writable || declaration.tls != tls {
            return Err(format!(
                "reserved JIT declaration snapshot data {name} storage flags mismatch"
            ));
        }
        Ok(declaration.id)
    }
}

impl JitModuleDeclarationSnapshot {
    fn from_module(jit_module: &JITModule) -> Self {
        Self {
            functions: jit_module
                .declarations()
                .get_functions()
                .map(|(id, declaration)| JitFunctionDeclarationSnapshot {
                    id,
                    name: declaration.name.clone(),
                    linkage: declaration.linkage,
                    signature: declaration.signature.clone(),
                })
                .collect(),
            data_objects: jit_module
                .declarations()
                .get_data_objects()
                .map(|(id, declaration)| JitDataDeclarationSnapshot {
                    id,
                    name: declaration.name.clone(),
                    linkage: declaration.linkage,
                    writable: declaration.writable,
                    tls: declaration.tls,
                })
                .collect(),
        }
    }

    fn function(&self, id: FuncId) -> Option<&JitFunctionDeclarationSnapshot> {
        self.functions
            .iter()
            .find(|declaration| declaration.id == id)
    }

    fn function_by_name(&self, name: &str) -> Option<&JitFunctionDeclarationSnapshot> {
        self.functions
            .iter()
            .find(|declaration| declaration.name.as_deref() == Some(name))
    }

    fn data(&self, id: DataId) -> Option<&JitDataDeclarationSnapshot> {
        self.data_objects
            .iter()
            .find(|declaration| declaration.id == id)
    }

    fn data_by_name(&self, name: &str) -> Option<&JitDataDeclarationSnapshot> {
        self.data_objects
            .iter()
            .find(|declaration| declaration.name.as_deref() == Some(name))
    }
}

#[derive(Clone)]
struct ProcessJitBatchFunction<'a> {
    function: BlockPyFunction<BlockPyModuleShape>,
    source: ProcessJitBatchFunctionSource<'a>,
}

#[derive(Clone)]
enum ProcessJitBatchFunctionSource<'a> {
    ExplicitInputs,
    BorrowedSharedState(&'a crate::module_type::SharedModuleState),
    OwnedSharedState(Arc<crate::module_type::SharedModuleState>),
}

impl ProcessJitBatchFunctionSource<'_> {
    fn shared_state(&self) -> Option<&crate::module_type::SharedModuleState> {
        match self {
            Self::ExplicitInputs => None,
            Self::BorrowedSharedState(shared_state) => Some(shared_state),
            Self::OwnedSharedState(shared_state) => Some(shared_state.as_ref()),
        }
    }
}

impl<'a> ProcessJitBatchFunction<'a> {
    fn into_static_owned(self) -> Result<ProcessJitBatchFunction<'static>, String> {
        let source = match self.source {
            ProcessJitBatchFunctionSource::ExplicitInputs => {
                ProcessJitBatchFunctionSource::ExplicitInputs
            }
            ProcessJitBatchFunctionSource::OwnedSharedState(shared_state) => {
                ProcessJitBatchFunctionSource::OwnedSharedState(shared_state)
            }
            ProcessJitBatchFunctionSource::BorrowedSharedState(_) => {
                return Err(format!(
                    "process JIT batch function {} id={} cannot be shared for foreground assist because it borrows module state",
                    self.function.names.qualname, self.function.function_id
                ));
            }
        };
        Ok(ProcessJitBatchFunction {
            function: self.function,
            source,
        })
    }
}

impl<'a> JitBatchPlan<'a> {
    fn into_static_owned(self) -> Result<JitBatchPlan<'static>, String> {
        let batch_functions = self
            .batch_functions
            .into_iter()
            .map(ProcessJitBatchFunction::into_static_owned)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(JitBatchPlan {
            root_function_id: self.root_function_id,
            env_config: self.env_config,
            batch_functions,
            function_indices_to_define: self.function_indices_to_define,
            function_compile_inputs: self.function_compile_inputs,
            compile_waiters: self.compile_waiters,
            module_declarations: self.module_declarations,
            isa: self.isa,
            module_plans: self.module_plans,
            predeclared: self.predeclared,
        })
    }
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
fn emit_jit_batch_codegen_log(
    root_function: &BlockPyFunction<BlockPyModuleShape>,
    direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    status: &str,
    failed_phase: &str,
    error: Option<&str>,
    batch_function_count: usize,
    functions_to_define_count: usize,
    batch_collect_elapsed: Duration,
    reservation_elapsed: Duration,
    codegen_elapsed: Duration,
    commit_elapsed: Duration,
    total_elapsed: Duration,
    worker_metrics: JitBatchWorkerMetrics,
) {
    let (module_name, package_name) = direct_call_resolver
        .map(|shared_state| {
            (
                shared_state.module_name.as_str(),
                shared_state.package_name.as_str(),
            )
        })
        .unwrap_or(("", ""));
    info!(
        target: "soac_jit_codegen",
        event = "soac.jit_batch_codegen",
        status,
        failed_phase,
        error = error.unwrap_or(""),
        module_name,
        package_name,
        root_function_id = %root_function.function_id,
        root_function_logical_id = root_function.function_id.local_function_id().as_u32(),
        root_function_qualname = root_function.names.qualname.as_str(),
        batch_function_count = u64::try_from(batch_function_count).unwrap_or(u64::MAX),
        functions_to_define_count = u64::try_from(functions_to_define_count).unwrap_or(u64::MAX),
        requested_worker_count = u64::try_from(worker_metrics.requested_worker_count).unwrap_or(u64::MAX),
        actual_worker_count = u64::try_from(worker_metrics.actual_worker_count).unwrap_or(u64::MAX),
        worker_function_count_min = u64::try_from(worker_metrics.worker_function_count_min).unwrap_or(u64::MAX),
        worker_function_count_max = u64::try_from(worker_metrics.worker_function_count_max).unwrap_or(u64::MAX),
        jit_batch_collect_us = duration_micros(batch_collect_elapsed),
        jit_batch_reservation_us = duration_micros(reservation_elapsed),
        jit_batch_codegen_us = duration_micros(codegen_elapsed),
        jit_batch_commit_us = duration_micros(commit_elapsed),
        jit_batch_total_us = duration_micros(total_elapsed),
        jit_batch_worker_total_sum_us = duration_micros(worker_metrics.worker_total_sum),
        jit_batch_worker_total_max_us = duration_micros(worker_metrics.worker_total_max),
        jit_batch_worker_setup_sum_us = duration_micros(worker_metrics.worker_setup_sum),
        jit_batch_worker_setup_max_us = duration_micros(worker_metrics.worker_setup_max),
        jit_batch_worker_compile_sum_us = duration_micros(worker_metrics.worker_compile_sum),
        jit_batch_worker_compile_max_us = duration_micros(worker_metrics.worker_compile_max),
        "jit_batch_codegen",
    );
}

pub(crate) struct ProcessJitEngine {
    env_config: SoacEnvConfig,
    module: ProcessJitModule,
    state: Mutex<ProcessJitState>,
    vectorcall_trampolines: Mutex<HashMap<usize, VectorcallEntryFn>>,
}

struct ProcessJitModule {
    jit_module: Mutex<JITModule>,
}

struct ProcessJitState {
    direct_functions: HashMap<RuntimeFunctionId, ProcessJitFunctionEntry>,
    module_constant_objects: HashMap<ModuleConstantObjectBindingKey, ModuleConstantObjectBinding>,
    scalar_counter_storage: HashMap<usize, ScalarCounterStorageBinding>,
    top_value_counter_storage: HashMap<usize, TopValueCounterStorageBinding>,
    next_direct_symbol_id: u64,
    next_planned_module_constant_binding_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ModuleConstantObjectBindingKey {
    SharedState(usize),
    ExplicitModule(usize),
    PlannedModule(u64),
}

impl std::fmt::Display for ModuleConstantObjectBindingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SharedState(key) => write!(f, "shared-state:{key}"),
            Self::ExplicitModule(key) => write!(f, "explicit-module:{key}"),
            Self::PlannedModule(id) => write!(f, "planned-module:{id}"),
        }
    }
}

#[derive(Clone)]
struct ModuleConstantObjectBinding {
    data_ids: Vec<DataId>,
}

#[derive(Clone, Copy)]
struct ScalarCounterStorageBinding {
    data_id: DataId,
    scalar_count: usize,
}

#[derive(Clone, Copy)]
struct TopValueCounterStorageBinding {
    data_id: DataId,
    top_value_count: usize,
}

#[derive(Clone)]
enum ProcessJitFunctionEntry {
    Declared {
        declared: DeclaredJitFunction,
        shape: ProcessJitFunctionShape,
    },
    Compiling {
        declared: DeclaredJitFunction,
        shape: ProcessJitFunctionShape,
        waiter: Arc<ProcessJitCompileWaiter>,
        work: Option<Arc<JitBatchWork>>,
    },
    Ready {
        declared: DeclaredJitFunction,
        shape: ProcessJitFunctionShape,
        compiled_handle: Arc<CompiledFunctionHandle>,
    },
}

#[derive(Clone, Eq, PartialEq)]
struct ProcessJitFunctionShape {
    qualname: String,
    param_count: usize,
}

impl ProcessJitFunctionShape {
    fn for_function(function: &BlockPyFunction<BlockPyModuleShape>) -> Self {
        Self {
            qualname: function.names.qualname.clone(),
            param_count: function.body_params().len(),
        }
    }
}

impl ProcessJitFunctionEntry {
    fn declared(&self) -> DeclaredJitFunction {
        match self {
            Self::Declared { declared, .. } => declared.clone(),
            Self::Compiling { declared, .. } => declared.clone(),
            Self::Ready { declared, .. } => declared.clone(),
        }
    }

    fn shape(&self) -> &ProcessJitFunctionShape {
        match self {
            Self::Declared { shape, .. }
            | Self::Compiling { shape, .. }
            | Self::Ready { shape, .. } => shape,
        }
    }

    fn ready_entry(&self) -> Option<(DeclaredJitFunction, Arc<CompiledFunctionHandle>)> {
        match self {
            Self::Ready {
                declared,
                compiled_handle,
                ..
            } => Some((declared.clone(), Arc::clone(compiled_handle))),
            Self::Declared { .. } | Self::Compiling { .. } => None,
        }
    }

    fn compile_waiter(&self) -> Option<Arc<ProcessJitCompileWaiter>> {
        match self {
            Self::Compiling { waiter, .. } => Some(Arc::clone(waiter)),
            Self::Declared { .. } | Self::Ready { .. } => None,
        }
    }

    fn compile_work(&self) -> Option<Arc<JitBatchWork>> {
        match self {
            Self::Compiling {
                work: Some(work), ..
            } => Some(Arc::clone(work)),
            Self::Compiling { work: None, .. } | Self::Declared { .. } | Self::Ready { .. } => None,
        }
    }
}

#[derive(Debug)]
struct ProcessJitCompileWaiter {
    result: Mutex<Option<Result<(), String>>>,
    ready: Condvar,
}

impl ProcessJitCompileWaiter {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn wait(&self) -> Result<(), String> {
        let mut result = self
            .result
            .lock()
            .map_err(|_| "process JIT compile waiter lock poisoned".to_string())?;
        loop {
            if let Some(result) = result.as_ref() {
                return result.clone();
            }
            result = self
                .ready
                .wait(result)
                .map_err(|_| "process JIT compile waiter lock poisoned".to_string())?;
        }
    }

    fn wait_timeout(&self, timeout: Duration) -> Result<Option<Result<(), String>>, String> {
        let result = self
            .result
            .lock()
            .map_err(|_| "process JIT compile waiter lock poisoned".to_string())?;
        if let Some(result) = result.as_ref() {
            return Ok(Some(result.clone()));
        }
        let (result, _) = self
            .ready
            .wait_timeout(result, timeout)
            .map_err(|_| "process JIT compile waiter lock poisoned".to_string())?;
        Ok(result.clone())
    }

    fn finish(&self, result: Result<(), String>) {
        if let Ok(mut slot) = self.result.lock() {
            if slot.is_none() {
                *slot = Some(result);
                self.ready.notify_all();
            }
        }
    }
}

fn wait_for_process_jit_compile(waiter: &ProcessJitCompileWaiter) -> Result<(), String> {
    Python::try_attach(|py| py.detach(|| waiter.wait())).unwrap_or_else(|| waiter.wait())
}

fn wait_for_process_jit_compile_timeout(
    waiter: &ProcessJitCompileWaiter,
    timeout: Duration,
) -> Result<Option<Result<(), String>>, String> {
    Python::try_attach(|py| py.detach(|| waiter.wait_timeout(timeout)))
        .unwrap_or_else(|| waiter.wait_timeout(timeout))
}

impl ProcessJitModule {
    fn new(compile_session: &crate::session::CompileSession) -> Result<Self, String> {
        Ok(Self {
            jit_module: Mutex::new(new_jit_module(compile_session)?),
        })
    }

    fn lock_for_serial_phase(&self) -> Result<MutexGuard<'_, JITModule>, String> {
        self.jit_module
            .lock()
            .map_err(|_| "process JIT module lock poisoned".to_string())
    }
}

impl ProcessJitState {
    fn new() -> Self {
        Self {
            direct_functions: HashMap::new(),
            module_constant_objects: HashMap::new(),
            scalar_counter_storage: HashMap::new(),
            top_value_counter_storage: HashMap::new(),
            next_direct_symbol_id: 0,
            next_planned_module_constant_binding_id: 0,
        }
    }

    fn next_planned_module_constant_binding(&mut self) -> (ModuleConstantObjectBindingKey, u64) {
        let id = self.next_planned_module_constant_binding_id;
        self.next_planned_module_constant_binding_id =
            self.next_planned_module_constant_binding_id.wrapping_add(1);
        (ModuleConstantObjectBindingKey::PlannedModule(id), id)
    }

    fn ensure_module_constant_objects(
        &mut self,
        jit_module: &mut JITModule,
        module_constant_ptrs: &[*mut ffi::PyObject],
        binding_key: ModuleConstantObjectBindingKey,
        symbol_prefix: &str,
    ) -> Result<Vec<DataId>, String> {
        if let Some(binding) = self.module_constant_objects.get(&binding_key) {
            if binding.data_ids.len() != module_constant_ptrs.len() {
                return Err(format!(
                    "module constant object count mismatch for module instance {}: {} != {}",
                    binding_key,
                    binding.data_ids.len(),
                    module_constant_ptrs.len()
                ));
            }
            return Ok(binding.data_ids.clone());
        }
        let data_ids = declare_module_constant_object_data_for_prefix(
            jit_module,
            symbol_prefix,
            module_constant_ptrs,
        )?;
        self.module_constant_objects.insert(
            binding_key,
            ModuleConstantObjectBinding {
                data_ids: data_ids.clone(),
            },
        );
        Ok(data_ids)
    }

    fn ensure_local_scalar_counter_storage(
        &mut self,
        jit_module: &mut JITModule,
        module: &BlockPyModule<BlockPyModuleShape>,
        scalar_counter_count: usize,
        instance_key: usize,
    ) -> Result<Option<DataId>, String> {
        if scalar_counter_count == 0 {
            return Ok(None);
        }
        if let Some(binding) = self.scalar_counter_storage.get(&instance_key).copied() {
            if binding.scalar_count != scalar_counter_count {
                return Err(format!(
                    "scalar counter storage length mismatch for module instance {}: {} != {}",
                    instance_key, binding.scalar_count, scalar_counter_count
                ));
            }
            return Ok(Some(binding.data_id));
        }
        let symbol = scalar_counter_storage_symbol_for_instance(module, instance_key);
        let data_id = define_scalar_counter_storage_data_for_symbol(
            jit_module,
            symbol.as_str(),
            scalar_counter_count,
        )?;
        self.scalar_counter_storage.insert(
            instance_key,
            ScalarCounterStorageBinding {
                data_id,
                scalar_count: scalar_counter_count,
            },
        );
        Ok(Some(data_id))
    }

    fn ensure_local_top_value_counter_storage(
        &mut self,
        jit_module: &mut JITModule,
        module: &BlockPyModule<BlockPyModuleShape>,
        top_value_counter_count: usize,
        instance_key: usize,
    ) -> Result<Option<DataId>, String> {
        if top_value_counter_count == 0 {
            return Ok(None);
        }
        if let Some(binding) = self.top_value_counter_storage.get(&instance_key).copied() {
            if binding.top_value_count != top_value_counter_count {
                return Err(format!(
                    "top-value counter storage length mismatch for module instance {}: {} != {}",
                    instance_key, binding.top_value_count, top_value_counter_count
                ));
            }
            return Ok(Some(binding.data_id));
        }
        let symbol = top_value_counter_storage_symbol_for_instance(module, instance_key);
        let data_id = define_top_value_counter_storage_data_for_symbol(
            jit_module,
            symbol.as_str(),
            top_value_counter_count,
        )?;
        self.top_value_counter_storage.insert(
            instance_key,
            TopValueCounterStorageBinding {
                data_id,
                top_value_count: top_value_counter_count,
            },
        );
        Ok(Some(data_id))
    }

    fn declare_direct_function(
        &mut self,
        jit_module: &mut JITModule,
        function: &BlockPyFunction<BlockPyModuleShape>,
        symbol_scope: Option<&str>,
    ) -> Result<DeclaredJitFunction, String> {
        let shape = ProcessJitFunctionShape::for_function(function);
        if let Some(entry) = self.direct_functions.get(&function.function_id) {
            if entry.shape() == &shape {
                return Ok(entry.declared());
            }
        }
        let owned_symbol_scope;
        let symbol_scope = if let Some(symbol_scope) = symbol_scope {
            symbol_scope
        } else {
            owned_symbol_scope =
                direct_function_symbol_scope(function.function_id, self.next_direct_symbol_id);
            self.next_direct_symbol_id = self.next_direct_symbol_id.wrapping_add(1);
            owned_symbol_scope.as_str()
        };
        let (_sig, declared) = declare_direct_function(jit_module, function, Some(symbol_scope))?;
        self.direct_functions.insert(
            function.function_id,
            ProcessJitFunctionEntry::Declared {
                declared: declared.clone(),
                shape,
            },
        );
        Ok(declared)
    }

    fn is_direct_function_ready(&self, function_id: RuntimeFunctionId) -> bool {
        self.direct_functions
            .get(&function_id)
            .is_some_and(|entry| entry.ready_entry().is_some())
    }

    fn direct_function_compile_waiter(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Option<Arc<ProcessJitCompileWaiter>> {
        let entry = self.direct_functions.get(&function.function_id)?;
        (entry.shape() == &ProcessJitFunctionShape::for_function(function))
            .then(|| entry.compile_waiter())
            .flatten()
    }

    fn promote_queued_direct_function_compile(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<bool, String> {
        let Some(entry) = self.direct_functions.get(&function.function_id) else {
            return Ok(false);
        };
        if entry.shape() != &ProcessJitFunctionShape::for_function(function) {
            return Ok(false);
        }
        let Some(work) = entry.compile_work() else {
            return Ok(false);
        };
        work.promote_function(function.function_id)
    }

    fn direct_function_compile_work(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Option<Arc<JitBatchWork>> {
        let entry = self.direct_functions.get(&function.function_id)?;
        (entry.shape() == &ProcessJitFunctionShape::for_function(function))
            .then(|| entry.compile_work())
            .flatten()
    }

    fn direct_function_dependency_waiter(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<Arc<ProcessJitCompileWaiter>> {
        let entry = self.direct_functions.get(&function_id)?;
        if entry.ready_entry().is_some() {
            return None;
        }
        entry.compile_waiter()
    }

    fn attach_direct_function_work(&mut self, plan: &JitBatchPlan<'_>, work: &Arc<JitBatchWork>) {
        for batch_function_index in &plan.function_indices_to_define {
            let function_id = plan.batch_functions[*batch_function_index]
                .function
                .function_id;
            let Some(waiter) = plan.compile_waiters.get(&function_id) else {
                continue;
            };
            let Some(entry) = self.direct_functions.get_mut(&function_id) else {
                continue;
            };
            let ProcessJitFunctionEntry::Compiling {
                waiter: entry_waiter,
                work: entry_work,
                ..
            } = entry
            else {
                continue;
            };
            if Arc::ptr_eq(waiter, entry_waiter) {
                *entry_work = Some(Arc::clone(work));
            }
        }
    }

    fn ready_direct_function(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Option<Arc<CompiledFunctionHandle>> {
        let entry = self.direct_functions.get(&function.function_id)?;
        (entry.shape() == &ProcessJitFunctionShape::for_function(function))
            .then(|| entry.ready_entry().map(|(_, handle)| handle))
            .flatten()
    }

    fn mark_direct_function_ready(
        &mut self,
        session: &Arc<crate::session::CompileSession>,
        function_id: RuntimeFunctionId,
        code_ptr: *const u8,
        default_code_ptr: *const u8,
        param_count: usize,
        deopt_table: Arc<RuntimeJitDeoptTable>,
    ) -> Result<Arc<CompiledFunctionHandle>, String> {
        let Some(entry) = self.direct_functions.get(&function_id) else {
            return Err(format!(
                "process JIT function {function_id} was defined before declaration"
            ));
        };
        debug_assert_eq!(deopt_table.function_id(), function_id);
        let declared = entry.declared();
        let shape = entry.shape().clone();
        let compile_waiter = entry.compile_waiter();
        let compiled_handle = Arc::new(CompiledFunctionHandle::from_direct_entry(
            session,
            code_ptr,
            default_code_ptr,
            param_count,
            deopt_table,
        ));
        self.direct_functions.insert(
            function_id,
            ProcessJitFunctionEntry::Ready {
                declared,
                shape,
                compiled_handle: Arc::clone(&compiled_handle),
            },
        );
        if let Some(waiter) = compile_waiter {
            waiter.finish(Ok(()));
        }
        Ok(compiled_handle)
    }

    fn fail_direct_function_batch(&mut self, plan: &JitBatchPlan<'_>, err: &str) {
        for (function_id, waiter) in &plan.compile_waiters {
            let Some(entry) = self.direct_functions.get(function_id) else {
                waiter.finish(Err(err.to_string()));
                continue;
            };
            let ProcessJitFunctionEntry::Compiling {
                declared,
                shape,
                waiter: entry_waiter,
                ..
            } = entry
            else {
                continue;
            };
            if !Arc::ptr_eq(waiter, entry_waiter) {
                continue;
            }
            let declared = declared.clone();
            let shape = shape.clone();
            self.direct_functions.insert(
                *function_id,
                ProcessJitFunctionEntry::Declared { declared, shape },
            );
            waiter.finish(Err(err.to_string()));
        }
    }
}

impl ProcessJitState {
    fn reserve_direct_function_compile_inputs(
        &mut self,
        jit_module: &mut JITModule,
        inputs: &DirectFunctionCompileInputs<'_>,
        batch_function: &ProcessJitBatchFunction<'_>,
    ) -> Result<ReservedJitFunctionCompileInputs, String> {
        if let Some(shared_state) = batch_function.source.shared_state() {
            let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state),
                Some(inputs.session.as_ref()),
            )?;
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let instance_key = shared_state.storage_instance_key();
            let scalar_counter_symbol =
                scalar_counter_storage_symbol_for_shared_state(shared_state);
            let scalar_counter_base_ptr = shared_state.scalar_counter_values_ptr();
            let scalar_counter_data_id = if scalar_counter_base_ptr.is_null() {
                None
            } else {
                register_jit_data_symbol(
                    scalar_counter_symbol.as_str(),
                    scalar_counter_base_ptr.cast::<u8>(),
                );
                Some(declare_scalar_counter_storage_import(
                    jit_module,
                    scalar_counter_symbol.as_str(),
                )?)
            };
            let top_value_counter_symbol =
                top_value_counter_storage_symbol_for_shared_state(shared_state);
            let top_value_counter_base_ptr = shared_state.top_value_counter_values_ptr();
            let top_value_counter_data_id = if top_value_counter_base_ptr.is_null() {
                None
            } else {
                register_jit_data_symbol(
                    top_value_counter_symbol.as_str(),
                    top_value_counter_base_ptr.cast::<u8>(),
                );
                Some(declare_top_value_counter_storage_import(
                    jit_module,
                    top_value_counter_symbol.as_str(),
                )?)
            };
            let module_constant_object_data_ids = self.ensure_module_constant_objects(
                jit_module,
                module_constant_ptrs.as_slice(),
                ModuleConstantObjectBindingKey::SharedState(instance_key),
                module_constant_symbol_prefix_for_shared_state(shared_state).as_str(),
            )?;
            let symbol_scope = direct_function_symbol_scope_for_shared_state(
                shared_state,
                batch_function.function.function_id,
            );
            let counted_refcount_helpers = build_counted_runtime_refcount_helpers(
                jit_module,
                inputs.session.env_config()?,
                &batch_function.function,
                shared_state.lowered_module.counter_defs.as_slice(),
                shared_state.counter_slots_by_id(),
                scalar_counter_data_id,
                Some(symbol_scope.as_str()),
            )?;
            predeclare_specialization_type_imports(jit_module, &specialization_profile)?;
            return Ok(ReservedJitFunctionCompileInputs {
                module_constant_ptrs,
                module_constant_owners: None,
                counter_slots_by_id: shared_state.counter_slots_by_id().to_vec(),
                module_constant_object_data_ids,
                scalar_counter_data_id,
                top_value_counter_data_id,
                counted_refcount_helpers,
                module_constant_binding_key: instance_key,
                symbol_scope: Some(symbol_scope),
            });
        }

        let (counter_slots_by_id, scalar_counter_count, top_value_count) =
            build_counter_storage_layout(inputs.counter_defs)?;
        let instance_key = inputs.module as *const BlockPyModule<BlockPyModuleShape> as usize;
        let scalar_counter_data_id = self.ensure_local_scalar_counter_storage(
            jit_module,
            inputs.module,
            scalar_counter_count,
            instance_key,
        )?;
        let top_value_counter_data_id = self.ensure_local_top_value_counter_storage(
            jit_module,
            inputs.module,
            top_value_count,
            instance_key,
        )?;
        let module_constant_ptrs = inputs.module_constant_ptrs.to_vec();
        let module_constant_object_data_ids = self.ensure_module_constant_objects(
            jit_module,
            module_constant_ptrs.as_slice(),
            ModuleConstantObjectBindingKey::ExplicitModule(instance_key),
            module_constant_symbol_prefix_for_instance(inputs.module, instance_key).as_str(),
        )?;
        let counted_refcount_helpers = build_counted_runtime_refcount_helpers(
            jit_module,
            inputs.session.env_config()?,
            &batch_function.function,
            inputs.counter_defs,
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            None,
        )?;
        let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
            inputs.direct_call_resolver,
            Some(inputs.session.as_ref()),
        )?;
        predeclare_specialization_type_imports(jit_module, &specialization_profile)?;
        Ok(ReservedJitFunctionCompileInputs {
            module_constant_ptrs,
            module_constant_owners: None,
            counter_slots_by_id: counter_slots_by_id.into_vec(),
            module_constant_object_data_ids,
            scalar_counter_data_id,
            top_value_counter_data_id,
            counted_refcount_helpers,
            module_constant_binding_key: instance_key,
            symbol_scope: None,
        })
    }

    fn reserve_direct_function_batch<'a>(
        &mut self,
        jit_module: &mut JITModule,
        inputs: &DirectFunctionCompileInputs<'a>,
        root_function: &BlockPyFunction<BlockPyModuleShape>,
        batch_functions: Vec<ProcessJitBatchFunction<'a>>,
    ) -> Result<ReservedDirectFunctionBatch<'a>, String> {
        if let Some(compiled_handle) = self.ready_direct_function(root_function) {
            return Ok(ReservedDirectFunctionBatch::Ready(compiled_handle));
        }
        if let Some(waiter) = self.direct_function_compile_waiter(root_function) {
            return Ok(ReservedDirectFunctionBatch::Compiling(waiter));
        }

        let mut predeclared = HashMap::new();
        let mut function_indices_to_define = Vec::new();
        let mut function_compile_inputs = HashMap::new();
        let mut compile_waiters = HashMap::new();
        for (index, batch_function) in batch_functions.iter().enumerate() {
            let function = &batch_function.function;
            let direct_symbol_scope = batch_function.source.shared_state().map(|shared_state| {
                direct_function_symbol_scope_for_shared_state(shared_state, function.function_id)
            });
            let declared =
                self.declare_direct_function(jit_module, function, direct_symbol_scope.as_deref())?;
            if self.is_direct_function_ready(function.function_id) {
                predeclared.insert(function.function_id, declared);
                continue;
            }
            if self.direct_function_compile_waiter(function).is_some() {
                predeclared.insert(function.function_id, declared);
                continue;
            }
            let waiter = Arc::new(ProcessJitCompileWaiter::new());
            let shape = ProcessJitFunctionShape::for_function(function);
            function_indices_to_define.push(index);
            function_compile_inputs.insert(
                index,
                self.reserve_direct_function_compile_inputs(jit_module, inputs, batch_function)?,
            );
            compile_waiters.insert(function.function_id, Arc::clone(&waiter));
            self.direct_functions.insert(
                function.function_id,
                ProcessJitFunctionEntry::Compiling {
                    declared: declared.clone(),
                    shape,
                    waiter,
                    work: None,
                },
            );
            predeclared.insert(function.function_id, declared);
        }
        predeclare_jit_runtime_imports(jit_module)?;

        Ok(ReservedDirectFunctionBatch::Reserved(JitBatchPlan {
            root_function_id: root_function.function_id,
            env_config: inputs.session.env_config()?.clone(),
            batch_functions,
            function_indices_to_define,
            function_compile_inputs,
            compile_waiters,
            module_declarations: JitModuleDeclarationSnapshot::from_module(jit_module),
            isa: CraneliftTargetConfig::runtime(inputs.session.env_config()?).build_isa()?,
            module_plans: HashMap::new(),
            predeclared,
        }))
    }

    fn compile_reserved_direct_function_batch_worker<'inputs, 'plan>(
        inputs: &DirectFunctionCompileInputs<'inputs>,
        plan: &JitBatchPlan<'plan>,
        batch_function_indices: &[usize],
    ) -> Result<JitBatchWorkerOutput, String> {
        let worker_start = Instant::now();
        let setup_start = Instant::now();
        let mut codegen_env = ReservedJitCodegenEnv {
            isa: Arc::clone(&plan.isa),
            declarations: &plan.module_declarations,
        };
        let setup = setup_start.elapsed();

        let compile_start = Instant::now();
        let compiled_functions = Self::compile_reserved_direct_function_batch_indices(
            &mut codegen_env,
            inputs,
            plan,
            batch_function_indices,
        )?;
        let compile = compile_start.elapsed();

        Ok(JitBatchWorkerOutput {
            compiled_functions,
            timing: JitBatchWorkerTiming {
                function_count: batch_function_indices.len(),
                total: worker_start.elapsed(),
                setup,
                compile,
            },
        })
    }

    fn compile_reserved_direct_function_batch_indices<'inputs, 'plan>(
        codegen_env: &mut impl JitCodegenEnv,
        inputs: &DirectFunctionCompileInputs<'inputs>,
        plan: &JitBatchPlan<'plan>,
        batch_function_indices: &[usize],
    ) -> Result<Vec<CompiledJitFunction>, String> {
        let mut compiled_functions = Vec::with_capacity(batch_function_indices.len());
        for batch_function_index in batch_function_indices {
            let compiled_function = Self::compile_reserved_direct_function_index(
                codegen_env,
                inputs,
                plan,
                *batch_function_index,
            )?;
            compiled_functions.push(compiled_function);
        }
        Ok(compiled_functions)
    }

    fn compile_reserved_direct_function_batch_worker_modules<'inputs, 'plan>(
        inputs: &DirectFunctionCompileInputs<'inputs>,
        plan: &JitBatchPlan<'plan>,
    ) -> Result<JitBatchCompileOutput, String> {
        let function_count = plan.function_indices_to_define.len();
        if function_count == 0 {
            return Ok(JitBatchCompileOutput {
                compiled_functions: Vec::new(),
                worker_metrics: JitBatchWorkerMetrics::default(),
            });
        }
        let worker_count = jit_batch_worker_count(function_count, &plan.env_config);
        if worker_count <= 1 {
            let worker_output = Self::compile_reserved_direct_function_batch_worker(
                inputs,
                plan,
                plan.function_indices_to_define.as_slice(),
            )?;
            let mut worker_metrics = JitBatchWorkerMetrics::new(worker_count);
            worker_metrics.record_worker(worker_output.timing);
            return Ok(JitBatchCompileOutput {
                compiled_functions: worker_output.compiled_functions,
                worker_metrics: worker_metrics.finish(),
            });
        }

        let chunk_size = function_count.div_ceil(worker_count);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for batch_function_indices in plan.function_indices_to_define.chunks(chunk_size) {
                handles.push(scope.spawn(move || {
                    let _guard = ProcessJitCompileGuard::enter();
                    Self::compile_reserved_direct_function_batch_worker(
                        inputs,
                        plan,
                        batch_function_indices,
                    )
                }));
            }

            let mut compiled_functions = Vec::with_capacity(function_count);
            let mut worker_metrics = JitBatchWorkerMetrics::new(worker_count);
            for handle in handles {
                match handle.join() {
                    Ok(Ok(mut worker_output)) => {
                        worker_metrics.record_worker(worker_output.timing);
                        compiled_functions.append(&mut worker_output.compiled_functions)
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(_) => return Err("process JIT batch codegen worker panicked".to_string()),
                }
            }
            Ok(JitBatchCompileOutput {
                compiled_functions,
                worker_metrics: worker_metrics.finish(),
            })
        })
    }

    fn compile_reserved_direct_function_index<'inputs, 'plan>(
        codegen_env: &mut impl JitCodegenEnv,
        inputs: &DirectFunctionCompileInputs<'inputs>,
        plan: &JitBatchPlan<'plan>,
        batch_function_index: usize,
    ) -> Result<CompiledJitFunction, String> {
        let batch_function = &plan.batch_functions[batch_function_index];
        let original_function = &batch_function.function;
        let reserved_inputs = plan
            .function_compile_inputs
            .get(&batch_function_index)
            .expect("reserved JIT batch function should have compile inputs");
        let function_module_constant_binding_key = reserved_inputs.module_constant_binding_key;
        let function_module_plan = plan
            .module_plans
            .get(&function_module_constant_binding_key)
            .expect("JIT module plan should be precomputed before worker codegen");
        let function_module = function_module_plan.module.as_ref();
        let function = function_module
            .callable_defs
            .iter()
            .find(|candidate| candidate.function_id == original_function.function_id)
            .ok_or_else(|| {
                format!(
                    "planned JIT module is missing function {} ({})",
                    original_function.function_id, original_function.names.qualname
                )
            })?;
        let planned_module_constants;
        let (function_module_constants, function_direct_call_resolver) =
            if let Some(shared_state) = batch_function.source.shared_state() {
                planned_module_constants = collect_codegen_constants_for_module_name(
                    shared_state.module_name.as_str(),
                    function_module,
                );
                (&planned_module_constants, Some(shared_state))
            } else {
                (inputs.module_constants, None)
            };
        let empty_blocks = Vec::new();
        let placeholder_blocks;
        let function_blocks = if function.function_id == plan.root_function_id {
            if inputs.blocks.len() == function.blocks.len() {
                inputs.blocks
            } else {
                empty_blocks.as_slice()
            }
        } else {
            placeholder_blocks =
                vec![std::ptr::null_mut::<std::ffi::c_void>(); function.blocks.len()];
            placeholder_blocks.as_slice()
        };
        let function_counter_defs = function_module.counter_defs.as_slice();
        let function_jit_local_plan = function_module_plan
            .locals
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let function_jit_deopt_resume_plan = function_module_plan
            .deopt_resume
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT deopt resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let function_deopt_table = Arc::new(RuntimeJitDeoptTable::from_plan_with_owned_constants(
            original_function,
            function_jit_deopt_resume_plan,
            reserved_inputs.module_constant_ptrs.as_slice(),
            reserved_inputs.module_constant_owners.clone(),
        )?);
        let built = build_cranelift_run_bb_specialized_function(
            codegen_env,
            function_blocks,
            function_module,
            function,
            &function_module_plan.value_facts,
            function_jit_local_plan,
            function_jit_deopt_resume_plan,
            function_module_constants,
            function_counter_defs,
            reserved_inputs.module_constant_object_data_ids.as_slice(),
            reserved_inputs.counter_slots_by_id.as_slice(),
            reserved_inputs.scalar_counter_data_id,
            reserved_inputs.top_value_counter_data_id,
            inputs.session.as_ref(),
            function_direct_call_resolver,
            reserved_inputs.symbol_scope.as_deref(),
            Some(&plan.predeclared),
            BuildSpecializedFunctionOptions {
                counted_refcount_helpers: Some(reserved_inputs.counted_refcount_helpers),
                ..BuildSpecializedFunctionOptions::default()
            },
        )
        .map_err(|err| {
            format!(
                "{err} [function={} id={}]",
                function.names.qualname, function.function_id
            )
        })?;
        let function_aliases = clif_function_display_aliases(
            &built.import_id_to_symbol,
            &built.local_func_id_to_symbol,
            &HashMap::new(),
            &built.direct_func_id_to_qualname,
        );
        let mut ctx = built.ctx;
        let main_id = built.main_id;
        let main_symbol = built.main_symbol;
        let default_adapter_id = built.default_adapter_id;
        let default_adapter_symbol = built.default_adapter_symbol;
        let clif_block_count = ctx.func.layout.blocks().count();
        let clif_inst_count = ctx.func.dfg.num_insts();
        let function_name =
            direct_function_backend_name(function, batch_function.source.shared_state());
        let compiled = compile_prepared_function_bytes_with_purpose_aliases(
            codegen_env,
            &plan.env_config,
            main_id,
            &mut ctx,
            function_name.as_str(),
            "failed to compile specialized jit run_bb function",
            Some(&function_aliases),
            Some(&built.block_roles),
        )
        .map_err(|err| {
            format!(
                "{err} [function={} id={}]",
                function.names.qualname, function.function_id
            )
        })?;
        codegen_env.codegen_clear_context(&mut ctx);
        let default_adapter_compiled = match (default_adapter_id, default_adapter_symbol.as_ref()) {
            (Some(default_adapter_id), Some(default_adapter_symbol)) => {
                let mut default_ctx = build_default_resolving_direct_adapter(
                    codegen_env,
                    function,
                    main_id,
                    default_adapter_id,
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        function.names.qualname, function.function_id
                    )
                })?;
                let compiled = compile_prepared_function_bytes(
                    codegen_env,
                    &plan.env_config,
                    default_adapter_id,
                    &mut default_ctx,
                    default_adapter_symbol.as_str(),
                    "failed to compile default-resolving direct adapter",
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        function.names.qualname, function.function_id
                    )
                })?;
                codegen_env.codegen_clear_context(&mut default_ctx);
                Some(compiled)
            }
            (None, None) => None,
            _ => {
                return Err(format!(
                    "default direct adapter declaration is inconsistent for function {} id={}",
                    function.names.qualname, function.function_id
                ));
            }
        };
        Ok(CompiledJitFunction {
            function_id: function.function_id,
            function_qualname: function.names.qualname.clone(),
            param_count: function.body_params().len(),
            main_id,
            main_symbol,
            default_adapter_id,
            default_adapter_symbol,
            stats: JitCodegenStats {
                clif_block_count,
                clif_inst_count,
                machine_code_size_bytes: compiled.artifact.code_size,
                machine_code_block_count: compiled.artifact.code_bb_offsets.len(),
                machine_code_edge_count: compiled.artifact.code_bb_edges.len(),
            },
            compiled,
            default_adapter_compiled,
            deopt_table: function_deopt_table,
        })
    }

    fn commit_compiled_direct_function_batch(
        &mut self,
        jit_module: &mut JITModule,
        session: &Arc<crate::session::CompileSession>,
        root_function: &BlockPyFunction<BlockPyModuleShape>,
        compiled_functions: Vec<CompiledJitFunction>,
    ) -> Result<DirectFunctionCompileResult, String> {
        let mut root_handle = None;
        let mut root_stats = None;
        for (function_id, compiled_handle, stats) in
            self.commit_compiled_direct_functions(jit_module, session, compiled_functions)?
        {
            if function_id == root_function.function_id {
                root_handle = Some(compiled_handle);
                root_stats = Some(stats);
            }
        }
        let handle = root_handle.ok_or_else(|| {
            format!(
                "process JIT batch did not define root function {} id={}",
                root_function.names.qualname, root_function.function_id
            )
        })?;
        Ok(DirectFunctionCompileResult {
            handle,
            compiled: true,
            stats: root_stats,
        })
    }

    fn commit_compiled_direct_functions(
        &mut self,
        jit_module: &mut JITModule,
        session: &Arc<crate::session::CompileSession>,
        compiled_functions: Vec<CompiledJitFunction>,
    ) -> Result<
        Vec<(
            RuntimeFunctionId,
            Arc<CompiledFunctionHandle>,
            JitCodegenStats,
        )>,
        String,
    > {
        for defined in &compiled_functions {
            define_compiled_function_bytes(
                jit_module,
                defined.main_id,
                &defined.compiled,
                "failed to define specialized jit run_bb function",
            )
            .map_err(|err| {
                format!(
                    "{err} [function={} id={}]",
                    defined.function_qualname, defined.function_id
                )
            })?;
            if let Some(default_adapter_compiled) = defined.default_adapter_compiled.as_ref() {
                let Some(default_adapter_id) = defined.default_adapter_id else {
                    return Err(format!(
                        "compiled default direct adapter is missing declaration for function {} id={}",
                        defined.function_qualname, defined.function_id
                    ));
                };
                define_compiled_function_bytes(
                    jit_module,
                    default_adapter_id,
                    default_adapter_compiled,
                    "failed to define default-resolving direct adapter",
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        defined.function_qualname, defined.function_id
                    )
                })?;
            }
        }
        jit_module
            .finalize_definitions()
            .map_err(|err| format!("failed to finalize specialized jit run_bb function: {err}"))?;

        let mut committed = Vec::with_capacity(compiled_functions.len());
        for defined in compiled_functions {
            let code_ptr = jit_module.get_finalized_function(defined.main_id);
            let default_code_ptr = defined
                .default_adapter_id
                .map(|default_adapter_id| jit_module.get_finalized_function(default_adapter_id))
                .unwrap_or(code_ptr);
            let compiled_handle = self.mark_direct_function_ready(
                session,
                defined.function_id,
                code_ptr,
                default_code_ptr,
                defined.param_count,
                Arc::clone(&defined.deopt_table),
            )?;
            let code_id = jitdump::record_code_load(
                &defined.main_symbol,
                code_ptr.cast::<u8>(),
                defined.compiled.artifact.code_size,
                jit_module.codegen_isa(),
                defined.compiled.artifact.systemv_unwind_info.as_ref(),
            )?;
            record_jit_bb_map(
                session.env_config()?,
                &defined.main_symbol,
                code_id,
                &defined.compiled.artifact,
                defined.function_id,
                &defined.function_qualname,
                "direct_function_body",
            );
            register_jit_signal_diagnostics(
                &defined.main_symbol,
                code_ptr.cast::<u8>(),
                &defined.compiled.artifact,
                defined.function_id,
                &defined.function_qualname,
                "direct_function_body",
            );
            if let (
                Some(default_adapter_id),
                Some(default_adapter_symbol),
                Some(default_adapter_compiled),
            ) = (
                defined.default_adapter_id,
                defined.default_adapter_symbol.as_ref(),
                defined.default_adapter_compiled.as_ref(),
            ) {
                let default_code_ptr = jit_module.get_finalized_function(default_adapter_id);
                let code_id = jitdump::record_code_load(
                    default_adapter_symbol,
                    default_code_ptr.cast::<u8>(),
                    default_adapter_compiled.artifact.code_size,
                    jit_module.codegen_isa(),
                    default_adapter_compiled
                        .artifact
                        .systemv_unwind_info
                        .as_ref(),
                )?;
                record_jit_bb_map(
                    session.env_config()?,
                    default_adapter_symbol,
                    code_id,
                    &default_adapter_compiled.artifact,
                    defined.function_id,
                    &defined.function_qualname,
                    "default_direct_adapter",
                );
                register_jit_signal_diagnostics(
                    default_adapter_symbol,
                    default_code_ptr.cast::<u8>(),
                    &default_adapter_compiled.artifact,
                    defined.function_id,
                    &defined.function_qualname,
                    "default_direct_adapter",
                );
            }
            committed.push((defined.function_id, compiled_handle, defined.stats));
        }
        Ok(committed)
    }
}

struct ProcessJitCompileGuard;

pub(crate) fn process_jit_is_currently_compiling() -> bool {
    PROCESS_JIT_COMPILE_DEPTH.with(|depth| depth.get() > 0)
}

impl ProcessJitCompileGuard {
    fn enter() -> Self {
        PROCESS_JIT_COMPILE_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for ProcessJitCompileGuard {
    fn drop(&mut self) {
        PROCESS_JIT_COMPILE_DEPTH.with(|depth| {
            let current = depth.get();
            debug_assert!(current > 0);
            depth.set(current.saturating_sub(1));
        });
    }
}

fn collect_process_jit_batch_functions<'a>(
    session: &Arc<crate::session::CompileSession>,
    root: &BlockPyFunction<BlockPyModuleShape>,
    direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
) -> Result<Vec<ProcessJitBatchFunction<'a>>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    let mut planned_inputs_by_module = HashMap::<usize, PlannedOptimizationInputs>::new();
    seen.insert(root.function_id);
    queue.push_back(ProcessJitBatchFunction {
        function: root.clone(),
        source: direct_call_resolver
            .map(ProcessJitBatchFunctionSource::BorrowedSharedState)
            .unwrap_or(ProcessJitBatchFunctionSource::ExplicitInputs),
    });
    while let Some(batch_function) = queue.pop_front() {
        if batch_function.function.execution_mode() != FunctionExecutionMode::Jit {
            continue;
        }
        let mut direct_targets = collect_call_direct_targets(&batch_function.function);
        for targets in planned_direct_call_targets_for_batch_function(
            session,
            &mut planned_inputs_by_module,
            &batch_function,
        )?
        .values()
        {
            direct_targets.extend(targets.iter().copied());
        }
        for function_id in direct_targets {
            if !seen.insert(function_id) {
                continue;
            }
            if let Some(function) =
                resolve_process_jit_batch_function(session, direct_call_resolver, function_id)?
            {
                if function.function.execution_mode() != FunctionExecutionMode::Jit {
                    continue;
                }
                queue.push_back(function);
            }
        }
        for function_id in collect_make_function_targets(&batch_function.function) {
            if seen.contains(&function_id) {
                continue;
            }
            if let Some(function) =
                resolve_process_jit_batch_function(session, direct_call_resolver, function_id)?
            {
                if function.function.execution_mode() != FunctionExecutionMode::Jit {
                    continue;
                }
                if is_synthetic_class_helper_function(&function.function) {
                    seen.insert(function_id);
                    queue.push_back(function);
                }
            }
        }
        out.push(batch_function);
    }
    Ok(out)
}

fn planned_direct_call_targets_for_batch_function(
    session: &Arc<crate::session::CompileSession>,
    planned_inputs_by_module: &mut HashMap<usize, PlannedOptimizationInputs>,
    batch_function: &ProcessJitBatchFunction<'_>,
) -> Result<HashMap<InstrId, Vec<RuntimeFunctionId>>, String> {
    let Some(shared_state) = batch_function.source.shared_state() else {
        return Ok(HashMap::new());
    };
    let module_key = shared_state.storage_instance_key();
    if !planned_inputs_by_module.contains_key(&module_key) {
        let planned_inputs = load_planned_optimization_inputs_for_runtime_state(
            Some(shared_state),
            Some(session.as_ref()),
            session.env_config()?,
            session.env_config()?.specialization_mode(),
        )?;
        planned_inputs_by_module.insert(module_key, planned_inputs);
    }
    Ok(planned_inputs_by_module
        .get(&module_key)
        .map(|planned_inputs| {
            planned_inputs.direct_call_targets_for_batch(batch_function.function.function_id)
        })
        .unwrap_or_default())
}

fn resolve_process_jit_batch_function<'a>(
    session: &Arc<crate::session::CompileSession>,
    direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
    function_id: RuntimeFunctionId,
) -> Result<Option<ProcessJitBatchFunction<'a>>, String> {
    if function_id == RuntimeFunctionId::global() {
        return Ok(None);
    }
    if let Some(shared_state) = direct_call_resolver
        && let Some(function) = shared_state.lookup_function(function_id).cloned()
    {
        return Ok(Some(ProcessJitBatchFunction {
            function,
            source: ProcessJitBatchFunctionSource::BorrowedSharedState(shared_state),
        }));
    }
    Ok(session
        .lookup_shared_function(function_id)?
        .map(|(shared_state, function)| ProcessJitBatchFunction {
            function,
            source: ProcessJitBatchFunctionSource::OwnedSharedState(shared_state),
        }))
}

impl ProcessJitEngine {
    pub(crate) fn new(compile_session: &crate::session::CompileSession) -> Result<Self, String> {
        Ok(Self {
            env_config: compile_session.env_config()?.clone(),
            module: ProcessJitModule::new(compile_session)?,
            state: Mutex::new(ProcessJitState::new()),
            vectorcall_trampolines: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn vectorcall_trampoline(
        &self,
        _compile_session: &crate::session::CompileSession,
        param_count: usize,
    ) -> Result<VectorcallEntryFn, String> {
        let mut trampolines = self
            .vectorcall_trampolines
            .lock()
            .map_err(|_| "process JIT vectorcall trampoline cache lock poisoned".to_string())?;
        if let Some(entry) = trampolines.get(&param_count).copied() {
            return Ok(entry);
        }

        let mut jit_module = self.module.lock_for_serial_phase()?;
        let symbol = format!("__soac_vectorcall_arity_{param_count}");
        let entry = define_shared_vectorcall_trampoline(
            &mut jit_module,
            &self.env_config,
            param_count,
            &symbol,
        )?;
        trampolines.insert(param_count, entry);
        Ok(entry)
    }

    pub(crate) fn lookup_ready_direct_function(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<Option<Arc<CompiledFunctionHandle>>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        Ok(state.ready_direct_function(function))
    }

    fn direct_function_needs_compile(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<bool, String> {
        if function.execution_mode() != FunctionExecutionMode::Jit {
            return Ok(false);
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        let Some(entry) = state.direct_functions.get(&function.function_id) else {
            return Ok(true);
        };
        if entry.shape() != &ProcessJitFunctionShape::for_function(function) {
            return Ok(true);
        }
        Ok(entry.ready_entry().is_none() && entry.compile_waiter().is_none())
    }

    fn fail_reserved_direct_function_batch(&self, plan: &JitBatchPlan<'_>, err: &str) {
        match self.state.lock() {
            Ok(mut state) => state.fail_direct_function_batch(plan, err),
            Err(_) => {
                let wait_error = format!("{err}; process JIT state lock poisoned");
                for waiter in plan.compile_waiters.values() {
                    waiter.finish(Err(wait_error.clone()));
                }
            }
        }
    }

    fn promote_queued_direct_function_compile(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<bool, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        state.promote_queued_direct_function_compile(function)
    }

    fn direct_function_compile_work(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<Option<Arc<JitBatchWork>>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        Ok(state.direct_function_compile_work(function))
    }

    fn direct_function_dependency_waiters(
        &self,
        dependency_ids: &[RuntimeFunctionId],
        locally_claimed_ids: &HashSet<RuntimeFunctionId>,
    ) -> Result<Vec<Arc<ProcessJitCompileWaiter>>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        Ok(dependency_ids
            .iter()
            .filter(|function_id| !locally_claimed_ids.contains(function_id))
            .filter_map(|function_id| state.direct_function_dependency_waiter(*function_id))
            .collect())
    }

    fn remove_globally_ready_functions(
        &self,
        function_ids: &mut HashSet<RuntimeFunctionId>,
    ) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        function_ids.retain(|function_id| !state.is_direct_function_ready(*function_id));
        Ok(())
    }

    fn assist_queued_direct_function_compile(
        &self,
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<bool, String> {
        let Some(work) = self.direct_function_compile_work(function)? else {
            return Ok(false);
        };
        let Some(context) = work.assist_context() else {
            return Ok(false);
        };
        let requested_indices = context.dependency_closure_indices(function.function_id);
        let Some(target_index) = context
            .index_by_function_id
            .get(&function.function_id)
            .copied()
        else {
            return Ok(false);
        };
        let claimed_indices = work.take_function_indices(&requested_indices)?;
        if !claimed_indices.contains(&target_index) {
            return Ok(false);
        }
        let claimed_ids: HashSet<_> = claimed_indices
            .iter()
            .map(|index| context.function_id_for_index(*index))
            .collect();
        let dependency_ids = context.dependency_closure_order(function.function_id);
        for waiter in self.direct_function_dependency_waiters(&dependency_ids, &claimed_ids)? {
            wait_for_process_jit_compile(&waiter)?;
        }
        let compiled_functions = match context.compile_indices(&claimed_indices) {
            Ok(compiled_functions) => compiled_functions,
            Err(err) => {
                self.fail_reserved_direct_function_batch(context.plan.as_ref(), &err);
                return Err(err);
            }
        };
        match self.commit_compiled_direct_function_group(&context.session, compiled_functions) {
            Ok(()) => Ok(true),
            Err(err) => {
                self.fail_reserved_direct_function_batch(context.plan.as_ref(), &err);
                Err(err)
            }
        }
    }

    fn commit_compiled_direct_function_group(
        &self,
        session: &Arc<crate::session::CompileSession>,
        compiled_functions: Vec<CompiledJitFunction>,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "process JIT state lock poisoned".to_string())?;
        let mut jit_module = self.module.lock_for_serial_phase()?;
        let _guard = ProcessJitCompileGuard::enter();
        state
            .commit_compiled_direct_functions(&mut jit_module, session, compiled_functions)
            .map(|_| ())
    }

    fn streaming_batch_direct_dependencies(
        plan: &JitBatchPlan<'_>,
    ) -> Result<HashMap<RuntimeFunctionId, HashSet<RuntimeFunctionId>>, String> {
        let function_ids_to_define: HashSet<_> = plan
            .function_indices_to_define
            .iter()
            .map(|index| plan.batch_functions[*index].function.function_id)
            .collect();
        let mut dependencies = HashMap::with_capacity(function_ids_to_define.len());
        for batch_function_index in &plan.function_indices_to_define {
            let batch_function = &plan.batch_functions[*batch_function_index];
            let function_id = batch_function.function.function_id;
            let mut direct_targets = collect_call_direct_targets(&batch_function.function);
            if let Some(reserved_inputs) = plan.function_compile_inputs.get(batch_function_index) {
                let module_plan = plan
                    .module_plans
                    .get(&reserved_inputs.module_constant_binding_key)
                    .ok_or_else(|| {
                        format!(
                            "missing planned JIT module for streaming dependencies of function {}",
                            function_id
                        )
                    })?;
                direct_targets.extend(collect_planned_typed_call_direct_targets(
                    module_plan,
                    function_id,
                )?);
            }
            direct_targets.retain(|target| function_ids_to_define.contains(target));
            dependencies.insert(function_id, direct_targets);
        }
        Ok(dependencies)
    }

    fn take_streaming_commit_ready_functions(
        pending: &mut HashMap<RuntimeFunctionId, CompiledJitFunction>,
        remaining: &HashSet<RuntimeFunctionId>,
        dependencies: &HashMap<RuntimeFunctionId, HashSet<RuntimeFunctionId>>,
    ) -> Vec<CompiledJitFunction> {
        let mut commit_ids: HashSet<_> = pending.keys().copied().collect();
        let mut changed = true;
        while changed {
            changed = false;
            let candidates: Vec<_> = commit_ids.iter().copied().collect();
            for function_id in candidates {
                let waits_for_uncompiled_dependency =
                    dependencies.get(&function_id).is_some_and(|deps| {
                        deps.iter()
                            .any(|dep| remaining.contains(dep) && !commit_ids.contains(dep))
                    });
                if waits_for_uncompiled_dependency {
                    commit_ids.remove(&function_id);
                    changed = true;
                }
            }
        }
        commit_ids
            .into_iter()
            .filter_map(|function_id| pending.remove(&function_id))
            .collect()
    }

    fn compile_reserved_direct_function_batch_streaming_commit<'inputs>(
        &self,
        inputs: &DirectFunctionCompileInputs<'inputs>,
        plan: Arc<JitBatchPlan<'static>>,
        dependencies: HashMap<RuntimeFunctionId, HashSet<RuntimeFunctionId>>,
        assist_context: Option<Arc<JitBatchAssistContext>>,
    ) -> Result<JitBatchStreamingCommitOutput, String> {
        let function_count = plan.function_indices_to_define.len();
        if function_count == 0 {
            return Ok(JitBatchStreamingCommitOutput {
                committed_function_count: 0,
                commit_elapsed: Duration::ZERO,
                worker_metrics: JitBatchWorkerMetrics::default(),
            });
        }
        let worker_count = jit_batch_worker_count(function_count, &plan.env_config);
        let mut remaining_function_ids: HashSet<_> = plan
            .function_indices_to_define
            .iter()
            .map(|index| plan.batch_functions[*index].function.function_id)
            .collect();
        let mut pending_functions = HashMap::new();
        let work = Arc::new(JitBatchWork::new(plan.as_ref(), assist_context));
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "process JIT state lock poisoned".to_string())?;
            state.attach_direct_function_work(plan.as_ref(), &work);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<JitBatchWorkerMessage>();

        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                let tx = tx.clone();
                let work = Arc::clone(&work);
                let stop = Arc::clone(&stop);
                let plan = Arc::clone(&plan);
                handles.push(scope.spawn(move || {
                    let _guard = ProcessJitCompileGuard::enter();
                    let worker_start = Instant::now();
                    let setup_start = Instant::now();
                    let mut codegen_env = ReservedJitCodegenEnv {
                        isa: Arc::clone(&plan.isa),
                        declarations: &plan.module_declarations,
                    };
                    let setup = setup_start.elapsed();
                    let compile_start = Instant::now();
                    let mut function_count = 0usize;

                    loop {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let batch_function_index = match work.pop_front() {
                            Ok(batch_function_index) => batch_function_index,
                            Err(_) => {
                                stop.store(true, Ordering::Release);
                                let _ = tx.send(JitBatchWorkerMessage::Compiled(Err(
                                    "process JIT background work queue lock poisoned".to_string(),
                                )));
                                break;
                            }
                        };
                        let Some(batch_function_index) = batch_function_index else {
                            break;
                        };
                        match ProcessJitState::compile_reserved_direct_function_index(
                            &mut codegen_env,
                            inputs,
                            plan.as_ref(),
                            batch_function_index,
                        ) {
                            Ok(compiled) => {
                                function_count += 1;
                                if tx
                                    .send(JitBatchWorkerMessage::Compiled(Ok(compiled)))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                stop.store(true, Ordering::Release);
                                let _ = tx.send(JitBatchWorkerMessage::Compiled(Err(err)));
                                break;
                            }
                        }
                    }
                    let _ = tx.send(JitBatchWorkerMessage::Done(JitBatchWorkerTiming {
                        function_count,
                        total: worker_start.elapsed(),
                        setup,
                        compile: compile_start.elapsed(),
                    }));
                }));
            }
            drop(tx);

            let mut committed_function_count = 0usize;
            let mut commit_elapsed = Duration::ZERO;
            let mut first_error = None;
            let mut worker_metrics = JitBatchWorkerMetrics::new(worker_count);
            for message in rx {
                match message {
                    JitBatchWorkerMessage::Compiled(Ok(compiled)) => {
                        if first_error.is_some() {
                            continue;
                        }
                        pending_functions.insert(compiled.function_id, compiled);
                        if let Err(err) =
                            self.remove_globally_ready_functions(&mut remaining_function_ids)
                        {
                            stop.store(true, Ordering::Release);
                            first_error = Some(err);
                            continue;
                        }
                        let ready_functions = Self::take_streaming_commit_ready_functions(
                            &mut pending_functions,
                            &remaining_function_ids,
                            &dependencies,
                        );
                        if !ready_functions.is_empty() {
                            let ready_function_ids: Vec<_> = ready_functions
                                .iter()
                                .map(|function| function.function_id)
                                .collect();
                            let commit_start = Instant::now();
                            match self.commit_compiled_direct_function_group(
                                inputs.session,
                                ready_functions,
                            ) {
                                Ok(()) => {
                                    commit_elapsed += commit_start.elapsed();
                                    committed_function_count += ready_function_ids.len();
                                    for function_id in ready_function_ids {
                                        remaining_function_ids.remove(&function_id);
                                    }
                                }
                                Err(err) => {
                                    commit_elapsed += commit_start.elapsed();
                                    stop.store(true, Ordering::Release);
                                    first_error = Some(err);
                                }
                            }
                        }
                    }
                    JitBatchWorkerMessage::Compiled(Err(err)) => {
                        stop.store(true, Ordering::Release);
                        if first_error.is_none() {
                            first_error = Some(err);
                        }
                    }
                    JitBatchWorkerMessage::Done(timing) => {
                        worker_metrics.record_worker(timing);
                    }
                }
            }
            for handle in handles {
                if handle.join().is_err() {
                    stop.store(true, Ordering::Release);
                    if first_error.is_none() {
                        first_error = Some("process JIT batch codegen worker panicked".to_string());
                    }
                }
            }
            if first_error.is_none()
                && let Err(err) = self.remove_globally_ready_functions(&mut remaining_function_ids)
            {
                first_error = Some(err);
            }
            if first_error.is_none() && !pending_functions.is_empty() {
                let ready_functions = Self::take_streaming_commit_ready_functions(
                    &mut pending_functions,
                    &remaining_function_ids,
                    &dependencies,
                );
                if !ready_functions.is_empty() {
                    let ready_function_ids: Vec<_> = ready_functions
                        .iter()
                        .map(|function| function.function_id)
                        .collect();
                    let commit_start = Instant::now();
                    match self
                        .commit_compiled_direct_function_group(inputs.session, ready_functions)
                    {
                        Ok(()) => {
                            commit_elapsed += commit_start.elapsed();
                            committed_function_count += ready_function_ids.len();
                        }
                        Err(err) => {
                            commit_elapsed += commit_start.elapsed();
                            first_error = Some(err);
                        }
                    }
                }
            }
            if first_error.is_none() && !pending_functions.is_empty() {
                first_error = Some(format!(
                    "process JIT streaming batch finished with {} uncommitted functions",
                    pending_functions.len()
                ));
            }
            if let Some(err) = first_error {
                Err(err)
            } else {
                Ok(JitBatchStreamingCommitOutput {
                    committed_function_count,
                    commit_elapsed,
                    worker_metrics: worker_metrics.finish(),
                })
            }
        })
    }

    pub(crate) fn start_background_compile_shared_module(
        &self,
        session: Arc<crate::session::CompileSession>,
        shared_state: Arc<crate::module_type::SharedModuleState>,
    ) -> Result<(), String> {
        let env_config = session.env_config()?;
        if !env_config.background_jit_enabled() {
            return Ok(());
        }
        if env_config.specialization_mode().is_some() {
            return Ok(());
        }
        if shared_state.lowered_module.callable_defs.is_empty() {
            return Ok(());
        }
        let module_name = shared_state.module_name.clone();
        let thread_name = format!("soac_jit-bg-{module_name}");
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                if let Err(err) = unsafe {
                    ProcessJitEngine::compile_shared_module_in_background(session, shared_state)
                } {
                    warn!(
                        target: "soac_jit_codegen",
                        module_name = %module_name,
                        error = %err,
                        "jit_background_module_compile"
                    );
                }
            })
            .map(|_| ())
            .map_err(|err| format!("failed to spawn process JIT background compile worker: {err}"))
    }

    unsafe fn compile_shared_module_in_background(
        session: Arc<crate::session::CompileSession>,
        shared_state: Arc<crate::module_type::SharedModuleState>,
    ) -> Result<(), String> {
        let batch_functions: Vec<_> = shared_state
            .lowered_module
            .callable_defs
            .iter()
            .filter(|function| {
                function.function_id != RuntimeFunctionId::global()
                    && function.execution_mode() == FunctionExecutionMode::Jit
            })
            .cloned()
            .map(|function| ProcessJitBatchFunction {
                function,
                source: ProcessJitBatchFunctionSource::OwnedSharedState(Arc::clone(&shared_state)),
            })
            .collect();
        if batch_functions.is_empty() {
            return Ok(());
        }
        let module_name = shared_state.module_name.clone();
        let start = Instant::now();
        let attempted = batch_functions.len();
        let mut first_error = None;
        let engine = session.process_jit()?;
        loop {
            let root_function = batch_functions
                .iter()
                .find_map(|batch_function| {
                    match engine.direct_function_needs_compile(&batch_function.function) {
                        Ok(true) => Some(Ok(batch_function.function.clone())),
                        Ok(false) => None,
                        Err(err) => Some(Err(err)),
                    }
                })
                .transpose()?;
            let Some(root_function) = root_function else {
                break;
            };
            let blocks = vec![std::ptr::null_mut::<std::ffi::c_void>(); root_function.blocks.len()];
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            match unsafe {
                engine.compile_direct_function_precollected_streaming_background(
                    &session,
                    blocks.as_slice(),
                    &shared_state.lowered_module,
                    &root_function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    module_constant_ptrs.as_slice(),
                    Some(shared_state.as_ref()),
                    batch_functions.clone(),
                )
            } {
                Ok(_result) => {}
                Err(err) => {
                    if first_error.is_none() {
                        first_error = Some(format!(
                            "{err} [background_function={} id={}]",
                            root_function.names.qualname, root_function.function_id
                        ));
                    }
                    break;
                }
            }
        }
        let elapsed = start.elapsed();
        let compiled_or_ready = batch_functions
            .iter()
            .filter(|batch_function| {
                engine
                    .direct_function_needs_compile(&batch_function.function)
                    .map(|needs_compile| !needs_compile)
                    .unwrap_or(false)
            })
            .count();
        match first_error {
            Some(err) => {
                warn!(
                    target: "soac_jit_codegen",
                    module_name = %module_name,
                    attempted_function_count = attempted,
                    compiled_or_ready_function_count = compiled_or_ready,
                    elapsed_us = duration_micros(elapsed),
                    error = %err,
                    "jit_background_module_compile_done"
                );
                Err(err)
            }
            None => {
                info!(
                    target: "soac_jit_codegen",
                    module_name = %module_name,
                    attempted_function_count = attempted,
                    compiled_or_ready_function_count = compiled_or_ready,
                    elapsed_us = duration_micros(elapsed),
                    "jit_background_module_compile_done"
                );
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn compile_direct_function_precollected_streaming_background<'a>(
        &self,
        session: &Arc<crate::session::CompileSession>,
        blocks: &[ObjPtr],
        module: &'a BlockPyModule<BlockPyModuleShape>,
        function: &BlockPyFunction<BlockPyModuleShape>,
        module_constants: &'a ModuleCodegenConstants,
        counter_defs: &'a [CounterDef],
        module_constant_ptrs: &[*mut ffi::PyObject],
        direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
        batch_functions: Vec<ProcessJitBatchFunction<'a>>,
    ) -> Result<(), String> {
        let total_start = Instant::now();
        let inputs = DirectFunctionCompileInputs {
            session,
            blocks,
            module,
            module_constants,
            counter_defs,
            module_constant_ptrs,
            direct_call_resolver,
        };
        let reservation_start = Instant::now();
        let reserved_batch_result = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "process JIT state lock poisoned".to_string())?;
            let mut jit_module = self.module.lock_for_serial_phase()?;
            let _guard = ProcessJitCompileGuard::enter();
            state.reserve_direct_function_batch(&mut jit_module, &inputs, function, batch_functions)
        };
        let reservation_elapsed = reservation_start.elapsed();
        let mut plan = match reserved_batch_result {
            Ok(ReservedDirectFunctionBatch::Ready(_)) => return Ok(()),
            Ok(ReservedDirectFunctionBatch::Compiling(waiter)) => {
                return wait_for_process_jit_compile(&waiter);
            }
            Ok(ReservedDirectFunctionBatch::Reserved(plan)) => plan,
            Err(err) => {
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "error",
                    "reserve",
                    Some(&err),
                    0,
                    0,
                    Duration::ZERO,
                    reservation_elapsed,
                    Duration::ZERO,
                    Duration::ZERO,
                    total_start.elapsed(),
                    JitBatchWorkerMetrics::default(),
                );
                return Err(err);
            }
        };
        if let Err(err) = plan.precompute_module_plans(&inputs) {
            emit_jit_batch_codegen_log(
                function,
                direct_call_resolver,
                "error",
                "plan",
                Some(&err),
                plan.batch_functions.len(),
                plan.function_indices_to_define.len(),
                Duration::ZERO,
                reservation_elapsed,
                Duration::ZERO,
                Duration::ZERO,
                total_start.elapsed(),
                JitBatchWorkerMetrics::default(),
            );
            self.fail_reserved_direct_function_batch(&plan, &err);
            return Err(err);
        }
        let refresh_start = Instant::now();
        let prepared_module_constant_bindings =
            match plan.prepare_planned_module_constant_bindings() {
                Ok(bindings) => bindings,
                Err(err) => {
                    emit_jit_batch_codegen_log(
                        function,
                        direct_call_resolver,
                        "error",
                        "constant-refresh",
                        Some(&err),
                        plan.batch_functions.len(),
                        plan.function_indices_to_define.len(),
                        Duration::ZERO,
                        reservation_elapsed,
                        refresh_start.elapsed(),
                        Duration::ZERO,
                        total_start.elapsed(),
                        JitBatchWorkerMetrics::default(),
                    );
                    self.fail_reserved_direct_function_batch(&plan, &err);
                    return Err(err);
                }
            };
        let refresh_result = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "process JIT state lock poisoned".to_string())?;
            let mut jit_module = self.module.lock_for_serial_phase()?;
            let _guard = ProcessJitCompileGuard::enter();
            let result = plan.bind_prepared_module_constant_bindings(
                prepared_module_constant_bindings,
                &mut state,
                &mut jit_module,
            );
            if result.is_ok() {
                plan.module_declarations = JitModuleDeclarationSnapshot::from_module(&jit_module);
            }
            result
        };
        if let Err(err) = refresh_result {
            emit_jit_batch_codegen_log(
                function,
                direct_call_resolver,
                "error",
                "constant-refresh",
                Some(&err),
                plan.batch_functions.len(),
                plan.function_indices_to_define.len(),
                Duration::ZERO,
                reservation_elapsed,
                refresh_start.elapsed(),
                Duration::ZERO,
                total_start.elapsed(),
                JitBatchWorkerMetrics::default(),
            );
            self.fail_reserved_direct_function_batch(&plan, &err);
            return Err(err);
        }
        let batch_function_count = plan.batch_functions.len();
        let functions_to_define_count = plan.function_indices_to_define.len();
        let dependencies = match Self::streaming_batch_direct_dependencies(&plan) {
            Ok(dependencies) => dependencies,
            Err(err) => {
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "error",
                    "dependency-analysis",
                    Some(&err),
                    batch_function_count,
                    functions_to_define_count,
                    Duration::ZERO,
                    reservation_elapsed,
                    Duration::ZERO,
                    Duration::ZERO,
                    total_start.elapsed(),
                    JitBatchWorkerMetrics::default(),
                );
                self.fail_reserved_direct_function_batch(&plan, &err);
                return Err(err);
            }
        };
        let assist_shared_state = plan.batch_functions.iter().find_map(|batch_function| {
            if let ProcessJitBatchFunctionSource::OwnedSharedState(shared_state) =
                &batch_function.source
            {
                Some(Arc::clone(shared_state))
            } else {
                None
            }
        });
        let compile_waiters_for_static_plan: Vec<_> =
            plan.compile_waiters.values().map(Arc::clone).collect();
        let plan = match plan.into_static_owned() {
            Ok(plan) => Arc::new(plan),
            Err(err) => {
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "error",
                    "foreground-assist",
                    Some(&err),
                    batch_function_count,
                    functions_to_define_count,
                    Duration::ZERO,
                    reservation_elapsed,
                    Duration::ZERO,
                    Duration::ZERO,
                    total_start.elapsed(),
                    JitBatchWorkerMetrics::default(),
                );
                for waiter in compile_waiters_for_static_plan {
                    waiter.finish(Err(err.clone()));
                }
                return Err(err);
            }
        };
        let assist_context = assist_shared_state.map(|shared_state| {
            Arc::new(JitBatchAssistContext::new(
                Arc::clone(session),
                Arc::clone(&plan),
                shared_state,
                blocks.to_vec(),
                module_constant_ptrs.to_vec(),
                dependencies.clone(),
            ))
        });
        let codegen_start = Instant::now();
        match self.compile_reserved_direct_function_batch_streaming_commit(
            &inputs,
            Arc::clone(&plan),
            dependencies,
            assist_context,
        ) {
            Ok(output) => {
                let codegen_elapsed = codegen_start.elapsed();
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "ok",
                    "",
                    None,
                    batch_function_count,
                    output.committed_function_count,
                    Duration::ZERO,
                    reservation_elapsed,
                    codegen_elapsed,
                    output.commit_elapsed,
                    total_start.elapsed(),
                    output.worker_metrics,
                );
                Ok(())
            }
            Err(err) => {
                let codegen_elapsed = codegen_start.elapsed();
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "error",
                    "codegen",
                    Some(&err),
                    batch_function_count,
                    functions_to_define_count,
                    Duration::ZERO,
                    reservation_elapsed,
                    codegen_elapsed,
                    Duration::ZERO,
                    total_start.elapsed(),
                    JitBatchWorkerMetrics::default(),
                );
                self.fail_reserved_direct_function_batch(plan.as_ref(), &err);
                Err(err)
            }
        }
    }

    pub(crate) unsafe fn compile_direct_function(
        &self,
        session: &Arc<crate::session::CompileSession>,
        blocks: &[ObjPtr],
        module: &BlockPyModule<BlockPyModuleShape>,
        function: &BlockPyFunction<BlockPyModuleShape>,
        module_constants: &ModuleCodegenConstants,
        counter_defs: &[CounterDef],
        module_constant_ptrs: &[*mut ffi::PyObject],
        direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    ) -> Result<DirectFunctionCompileResult, String> {
        if function.execution_mode() != FunctionExecutionMode::Jit {
            return Err(format!(
                "function {} id={} is marked for interpreted execution",
                function.names.qualname, function.function_id
            ));
        }
        let mut background_wait_errors = 0usize;
        loop {
            match self.compile_direct_function_once(
                session,
                blocks,
                module,
                function,
                module_constants,
                counter_defs,
                module_constant_ptrs,
                direct_call_resolver,
            ) {
                Ok(DirectFunctionCompileAttempt::Done(result)) => return Ok(result),
                Ok(DirectFunctionCompileAttempt::Wait(waiter)) => {
                    let wait_result = loop {
                        if !self.assist_queued_direct_function_compile(function)? {
                            self.promote_queued_direct_function_compile(function)?;
                        }
                        match wait_for_process_jit_compile_timeout(
                            &waiter,
                            Duration::from_millis(10),
                        ) {
                            Ok(Some(result)) => break result,
                            Ok(None) => {}
                            Err(err) => break Err(err),
                        }
                    };
                    if let Some(handle) = self.lookup_ready_direct_function(function)? {
                        return Ok(DirectFunctionCompileResult {
                            handle,
                            compiled: false,
                            stats: None,
                        });
                    }
                    if let Err(err) = wait_result {
                        background_wait_errors += 1;
                        if background_wait_errors > 4 {
                            return Err(format!(
                                "process JIT background compile repeatedly failed for function {} id={}: {err}",
                                function.names.qualname, function.function_id
                            ));
                        }
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

    unsafe fn compile_direct_function_once(
        &self,
        session: &Arc<crate::session::CompileSession>,
        blocks: &[ObjPtr],
        module: &BlockPyModule<BlockPyModuleShape>,
        function: &BlockPyFunction<BlockPyModuleShape>,
        module_constants: &ModuleCodegenConstants,
        counter_defs: &[CounterDef],
        module_constant_ptrs: &[*mut ffi::PyObject],
        direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
    ) -> Result<DirectFunctionCompileAttempt, String> {
        let total_start = Instant::now();
        let batch_collect_start = Instant::now();
        let batch_functions =
            match collect_process_jit_batch_functions(session, function, direct_call_resolver) {
                Ok(batch_functions) => batch_functions,
                Err(err) => {
                    emit_jit_batch_codegen_log(
                        function,
                        direct_call_resolver,
                        "error",
                        "collect",
                        Some(&err),
                        0,
                        0,
                        batch_collect_start.elapsed(),
                        Duration::ZERO,
                        Duration::ZERO,
                        Duration::ZERO,
                        total_start.elapsed(),
                        JitBatchWorkerMetrics::default(),
                    );
                    return Err(err);
                }
            };
        let batch_collect_elapsed = batch_collect_start.elapsed();
        self.compile_direct_function_precollected_once(
            session,
            blocks,
            module,
            function,
            module_constants,
            counter_defs,
            module_constant_ptrs,
            direct_call_resolver,
            batch_functions,
            batch_collect_elapsed,
            total_start,
        )
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn compile_direct_function_precollected_once<'a>(
        &self,
        session: &Arc<crate::session::CompileSession>,
        blocks: &[ObjPtr],
        module: &'a BlockPyModule<BlockPyModuleShape>,
        function: &BlockPyFunction<BlockPyModuleShape>,
        module_constants: &'a ModuleCodegenConstants,
        counter_defs: &'a [CounterDef],
        module_constant_ptrs: &[*mut ffi::PyObject],
        direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
        batch_functions: Vec<ProcessJitBatchFunction<'a>>,
        batch_collect_elapsed: Duration,
        total_start: Instant,
    ) -> Result<DirectFunctionCompileAttempt, String> {
        let inputs = DirectFunctionCompileInputs {
            session,
            blocks,
            module,
            module_constants,
            counter_defs,
            module_constant_ptrs,
            direct_call_resolver,
        };
        let reservation_start = Instant::now();
        let reserved_batch_result = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "process JIT state lock poisoned".to_string())?;
            let mut jit_module = self.module.lock_for_serial_phase()?;
            let _guard = ProcessJitCompileGuard::enter();
            state.reserve_direct_function_batch(&mut jit_module, &inputs, function, batch_functions)
        };
        let reservation_elapsed = reservation_start.elapsed();
        let mut plan = match reserved_batch_result {
            Ok(ReservedDirectFunctionBatch::Ready(handle)) => {
                return Ok(DirectFunctionCompileResult {
                    handle,
                    compiled: false,
                    stats: None,
                }
                .into());
            }
            Ok(ReservedDirectFunctionBatch::Compiling(waiter)) => {
                return Ok(DirectFunctionCompileAttempt::Wait(waiter));
            }
            Ok(ReservedDirectFunctionBatch::Reserved(plan)) => plan,
            Err(err) => {
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "error",
                    "reserve",
                    Some(&err),
                    0,
                    0,
                    batch_collect_elapsed,
                    reservation_elapsed,
                    Duration::ZERO,
                    Duration::ZERO,
                    total_start.elapsed(),
                    JitBatchWorkerMetrics::default(),
                );
                return Err(err);
            }
        };
        if let Err(err) = plan.precompute_module_plans(&inputs) {
            emit_jit_batch_codegen_log(
                function,
                direct_call_resolver,
                "error",
                "plan",
                Some(&err),
                plan.batch_functions.len(),
                plan.function_indices_to_define.len(),
                batch_collect_elapsed,
                reservation_elapsed,
                Duration::ZERO,
                Duration::ZERO,
                total_start.elapsed(),
                JitBatchWorkerMetrics::default(),
            );
            self.fail_reserved_direct_function_batch(&plan, &err);
            return Err(err);
        }
        let refresh_start = Instant::now();
        let prepared_module_constant_bindings =
            match plan.prepare_planned_module_constant_bindings() {
                Ok(bindings) => bindings,
                Err(err) => {
                    emit_jit_batch_codegen_log(
                        function,
                        direct_call_resolver,
                        "error",
                        "constant-refresh",
                        Some(&err),
                        plan.batch_functions.len(),
                        plan.function_indices_to_define.len(),
                        batch_collect_elapsed,
                        reservation_elapsed,
                        refresh_start.elapsed(),
                        Duration::ZERO,
                        total_start.elapsed(),
                        JitBatchWorkerMetrics::default(),
                    );
                    self.fail_reserved_direct_function_batch(&plan, &err);
                    return Err(err);
                }
            };
        let refresh_result = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "process JIT state lock poisoned".to_string())?;
            let mut jit_module = self.module.lock_for_serial_phase()?;
            let _guard = ProcessJitCompileGuard::enter();
            let result = plan.bind_prepared_module_constant_bindings(
                prepared_module_constant_bindings,
                &mut state,
                &mut jit_module,
            );
            if result.is_ok() {
                plan.module_declarations = JitModuleDeclarationSnapshot::from_module(&jit_module);
            }
            result
        };
        if let Err(err) = refresh_result {
            emit_jit_batch_codegen_log(
                function,
                direct_call_resolver,
                "error",
                "constant-refresh",
                Some(&err),
                plan.batch_functions.len(),
                plan.function_indices_to_define.len(),
                batch_collect_elapsed,
                reservation_elapsed,
                refresh_start.elapsed(),
                Duration::ZERO,
                total_start.elapsed(),
                JitBatchWorkerMetrics::default(),
            );
            self.fail_reserved_direct_function_batch(&plan, &err);
            return Err(err);
        }
        let batch_function_count = plan.batch_functions.len();
        let functions_to_define_count = plan.function_indices_to_define.len();
        let codegen_start = Instant::now();
        let batch_output = {
            let _guard = ProcessJitCompileGuard::enter();
            match ProcessJitState::compile_reserved_direct_function_batch_worker_modules(
                &inputs, &plan,
            ) {
                Ok(batch_output) => batch_output,
                Err(err) => {
                    let codegen_elapsed = codegen_start.elapsed();
                    emit_jit_batch_codegen_log(
                        function,
                        direct_call_resolver,
                        "error",
                        "codegen",
                        Some(&err),
                        batch_function_count,
                        functions_to_define_count,
                        batch_collect_elapsed,
                        reservation_elapsed,
                        codegen_elapsed,
                        Duration::ZERO,
                        total_start.elapsed(),
                        JitBatchWorkerMetrics::default(),
                    );
                    self.fail_reserved_direct_function_batch(&plan, &err);
                    return Err(err);
                }
            }
        };
        let codegen_elapsed = codegen_start.elapsed();
        let worker_metrics = batch_output.worker_metrics;
        let commit_start = Instant::now();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => {
                let err = "process JIT state lock poisoned".to_string();
                self.fail_reserved_direct_function_batch(&plan, &err);
                return Err(err);
            }
        };
        let mut jit_module = match self.module.lock_for_serial_phase() {
            Ok(jit_module) => jit_module,
            Err(err) => {
                drop(state);
                self.fail_reserved_direct_function_batch(&plan, &err);
                return Err(err);
            }
        };
        let _guard = ProcessJitCompileGuard::enter();
        let commit_result = state.commit_compiled_direct_function_batch(
            &mut jit_module,
            session,
            function,
            batch_output.compiled_functions,
        );
        let commit_elapsed = commit_start.elapsed();
        drop(jit_module);
        drop(state);
        match commit_result {
            Ok(result) => {
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "ok",
                    "",
                    None,
                    batch_function_count,
                    functions_to_define_count,
                    batch_collect_elapsed,
                    reservation_elapsed,
                    codegen_elapsed,
                    commit_elapsed,
                    total_start.elapsed(),
                    worker_metrics,
                );
                Ok(result.into())
            }
            Err(err) => {
                emit_jit_batch_codegen_log(
                    function,
                    direct_call_resolver,
                    "error",
                    "commit",
                    Some(&err),
                    batch_function_count,
                    functions_to_define_count,
                    batch_collect_elapsed,
                    reservation_elapsed,
                    codegen_elapsed,
                    commit_elapsed,
                    total_start.elapsed(),
                    worker_metrics,
                );
                self.fail_reserved_direct_function_batch(&plan, &err);
                Err(err)
            }
        }
    }
}

enum DirectFunctionCompileAttempt {
    Done(DirectFunctionCompileResult),
    Wait(Arc<ProcessJitCompileWaiter>),
}

impl From<DirectFunctionCompileResult> for DirectFunctionCompileAttempt {
    fn from(result: DirectFunctionCompileResult) -> Self {
        Self::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessJitModule, ProcessJitState};
    use crate::jit::RuntimeJitDeoptTable;
    use soac_core::block_py::{
        BlockPyFunction, FunctionKind, FunctionName, ModuleNameGen, Param, ParamKind, ParamSpec,
    };
    use soac_ir_blockpy::BlockPyModuleShape;
    use std::sync::Arc;

    fn test_function() -> BlockPyFunction<BlockPyModuleShape> {
        let module_name_gen = ModuleNameGen::new(0);
        let name_gen = module_name_gen.next_function_name_gen();
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new("test", "test", "test", "test"),
            kind: FunctionKind::Function,
            execution_mode: Default::default(),
            params: ParamSpec::default(),
            body_params: None,
            public_scope: None,
            blocks: vec![],
            doc: None,
            public_storage_layout: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    #[test]
    fn process_jit_registry_does_not_reuse_colliding_function_ids_with_different_shapes() {
        let compile_session = crate::session::CompileSession::new();
        let module =
            ProcessJitModule::new(&compile_session).expect("process JIT module should initialize");
        let mut jit_module = module
            .lock_for_serial_phase()
            .expect("process JIT module should lock");
        let mut state = ProcessJitState::new();
        let first = test_function();
        let mut second = test_function();
        second.params.params.push(Param {
            name: "x".into(),
            kind: ParamKind::Any,
            has_default: false,
        });

        let first_decl = state
            .declare_direct_function(&mut jit_module, &first, None)
            .expect("first function should declare");
        let first_decl_again = state
            .declare_direct_function(&mut jit_module, &first, None)
            .expect("same shape should reuse declaration");
        assert_eq!(first_decl.symbol, first_decl_again.symbol);

        let session = Arc::new(crate::session::CompileSession::new());
        let first_handle = state
            .mark_direct_function_ready(
                &session,
                first.function_id,
                1usize as *const u8,
                1usize as *const u8,
                first.params.len(),
                Arc::new(RuntimeJitDeoptTable {
                    function_id: first.function_id,
                    function: Box::new(first.clone()),
                    module_constant_ptrs: Vec::new(),
                    points: Vec::new(),
                }),
            )
            .expect("first function should mark ready");
        let ready_handle = state
            .ready_direct_function(&first)
            .expect("first function should be ready");
        assert!(Arc::ptr_eq(&first_handle, &ready_handle));
        assert!(state.ready_direct_function(&second).is_none());

        let second_decl = state
            .declare_direct_function(&mut jit_module, &second, None)
            .expect("colliding function id with different shape should redeclare");
        assert_ne!(first_decl.symbol, second_decl.symbol);
    }
}
