use crate::jit::{JitModulePlan, PlannedOptimizationInputs, ProcessJitEngine};
use crate::module_type::SharedModuleState;
use soac_config::{SoacEnvConfig, SpecializationMode};
use soac_core::block_py::{BlockPyFunction, ModuleNameGen, RuntimeFunctionId};
use soac_ir_blockpy::BlockPyModuleShape;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static NEXT_COMPILE_SESSION_ID: AtomicU32 = AtomicU32::new(1);
static PROCESS_COMPILE_SESSION: OnceLock<Arc<CompileSession>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CompileSessionId(u32);

impl CompileSessionId {
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

pub fn allocate_compile_session_id() -> CompileSessionId {
    CompileSessionId(NEXT_COMPILE_SESSION_ID.fetch_add(1, Ordering::Relaxed))
}

pub struct CompileSession {
    id: CompileSessionId,
    next_module_id: AtomicU32,
    shared_module_registry_epoch: AtomicU64,
    shared_module_states: Mutex<SharedModuleStateRegistry>,
    planned_optimization_inputs:
        Mutex<HashMap<PlannedOptimizationInputsCacheKey, Arc<PlannedOptimizationInputsResult>>>,
    shared_typed_module_plans:
        Mutex<HashMap<SharedTypedModulePlanCacheKey, Arc<SharedTypedModulePlanResult>>>,
    process_jit: OnceLock<Result<ProcessJitEngine, String>>,
    env_config: OnceLock<Result<SoacEnvConfig, String>>,
}

type PlannedOptimizationInputsResult = Result<PlannedOptimizationInputs, String>;
type SharedTypedModulePlanResult = Result<Arc<JitModulePlan>, String>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PlannedOptimizationInputsCacheKey {
    module_storage_instance_key: usize,
    shared_module_registry_epoch: u64,
    counter_dump_path: PathBuf,
    specialization_mode: CachedSpecializationMode,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CachedSpecializationMode {
    Verify,
    Apply,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CachedSharedTypedModulePlanMode {
    Profile,
    Verify,
    Apply,
}

impl PlannedOptimizationInputsCacheKey {
    pub(crate) fn new(
        module_storage_instance_key: usize,
        shared_module_registry_epoch: u64,
        counter_dump_path: PathBuf,
        specialization_mode: SpecializationMode,
    ) -> Option<Self> {
        let specialization_mode = CachedSpecializationMode::new(specialization_mode)?;
        Some(Self {
            module_storage_instance_key,
            shared_module_registry_epoch,
            counter_dump_path,
            specialization_mode,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SharedTypedModulePlanCacheKey {
    module_storage_instance_key: usize,
    shared_module_registry_epoch: u64,
    counter_dump_path: Option<PathBuf>,
    specialization_mode: CachedSharedTypedModulePlanMode,
    behavior_change_indexed_stores: bool,
    profiled_cold_blocks: bool,
    guard_miss_deopt: bool,
}

impl SharedTypedModulePlanCacheKey {
    pub(crate) fn new(
        module_storage_instance_key: usize,
        shared_module_registry_epoch: u64,
        counter_dump_path: Option<PathBuf>,
        specialization_mode: SpecializationMode,
        behavior_change_indexed_stores: bool,
        profiled_cold_blocks: bool,
        guard_miss_deopt: bool,
    ) -> Option<Self> {
        let specialization_mode = CachedSharedTypedModulePlanMode::from(specialization_mode);
        if specialization_mode != CachedSharedTypedModulePlanMode::Profile
            && counter_dump_path.is_none()
        {
            return None;
        }
        Some(Self {
            module_storage_instance_key,
            shared_module_registry_epoch,
            counter_dump_path,
            specialization_mode,
            behavior_change_indexed_stores,
            profiled_cold_blocks,
            guard_miss_deopt,
        })
    }
}

impl From<SpecializationMode> for CachedSharedTypedModulePlanMode {
    fn from(specialization_mode: SpecializationMode) -> Self {
        match specialization_mode {
            SpecializationMode::Profile => Self::Profile,
            SpecializationMode::Verify => Self::Verify,
            SpecializationMode::Apply => Self::Apply,
        }
    }
}

impl CachedSpecializationMode {
    fn new(specialization_mode: SpecializationMode) -> Option<Self> {
        match specialization_mode {
            SpecializationMode::Verify => Some(Self::Verify),
            SpecializationMode::Apply => Some(Self::Apply),
            SpecializationMode::Profile => None,
        }
    }
}

#[derive(Default)]
struct SharedModuleStateRegistry {
    retained: Vec<Arc<SharedModuleState>>,
    by_module_id: HashMap<u32, usize>,
    by_module_identity: HashMap<(String, u64), usize>,
}

impl SharedModuleStateRegistry {
    fn retain(&mut self, shared_state: Arc<SharedModuleState>) {
        let module_id = shared_state.lowered_module.module_name_gen.module_id();
        let module_identity = (shared_state.module_name.clone(), shared_state.source_hash());
        let index = self.retained.len();
        self.retained.push(shared_state);
        self.by_module_id.insert(module_id, index);
        self.by_module_identity.insert(module_identity, index);
    }

    fn for_function_id(&self, function_id: RuntimeFunctionId) -> Option<Arc<SharedModuleState>> {
        let index = self
            .by_module_id
            .get(&function_id.runtime_module_id().as_u32())
            .copied()?;
        self.retained.get(index).cloned()
    }

    fn for_module_identity(
        &self,
        module_name: &str,
        source_hash: u64,
    ) -> Option<Arc<SharedModuleState>> {
        let index = self
            .by_module_identity
            .get(&(module_name.to_string(), source_hash))
            .copied()?;
        self.retained.get(index).cloned()
    }

    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.retained.len()
    }
}

impl CompileSession {
    pub fn new() -> Self {
        Self {
            id: allocate_compile_session_id(),
            next_module_id: AtomicU32::new(1),
            shared_module_registry_epoch: AtomicU64::new(0),
            shared_module_states: Mutex::new(SharedModuleStateRegistry::default()),
            planned_optimization_inputs: Mutex::new(HashMap::new()),
            shared_typed_module_plans: Mutex::new(HashMap::new()),
            process_jit: OnceLock::new(),
            env_config: OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_env_config(env_config: SoacEnvConfig) -> Self {
        let session = Self::new();
        let _ = session.env_config.set(Ok(env_config));
        session
    }

    pub fn process() -> Arc<Self> {
        Arc::clone(PROCESS_COMPILE_SESSION.get_or_init(|| Arc::new(Self::new())))
    }

    pub fn id(&self) -> CompileSessionId {
        self.id
    }

    pub fn module_name_gen(&self) -> ModuleNameGen {
        ModuleNameGen::new(self.next_module_id.fetch_add(1, Ordering::Relaxed))
    }

    pub fn env_config(&self) -> Result<&SoacEnvConfig, String> {
        match self.env_config.get_or_init(SoacEnvConfig::from_env) {
            Ok(config) => Ok(config),
            Err(err) => Err(err.clone()),
        }
    }

    pub(crate) fn process_jit(&self) -> Result<&ProcessJitEngine, String> {
        match self.process_jit.get_or_init(|| ProcessJitEngine::new(self)) {
            Ok(engine) => Ok(engine),
            Err(err) => Err(err.clone()),
        }
    }

    pub(crate) fn retain_shared_module_state(
        &self,
        shared_state: Arc<SharedModuleState>,
    ) -> Result<(), String> {
        self.shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .retain(shared_state);
        self.shared_module_registry_epoch
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn shared_module_registry_epoch(&self) -> u64 {
        self.shared_module_registry_epoch.load(Ordering::Relaxed)
    }

    pub(crate) fn cached_planned_optimization_inputs(
        &self,
        key: PlannedOptimizationInputsCacheKey,
        build: impl FnOnce() -> PlannedOptimizationInputsResult,
    ) -> PlannedOptimizationInputsResult {
        if let Some(cached) = self
            .planned_optimization_inputs
            .lock()
            .map_err(|_| {
                "compile session planned optimization input cache lock poisoned".to_string()
            })?
            .get(&key)
            .cloned()
        {
            return clone_planned_optimization_inputs_result(cached.as_ref());
        }

        let built = Arc::new(build());
        let cached = {
            let mut cache = self.planned_optimization_inputs.lock().map_err(|_| {
                "compile session planned optimization input cache lock poisoned".to_string()
            })?;
            cache
                .entry(key)
                .or_insert_with(|| Arc::clone(&built))
                .clone()
        };
        clone_planned_optimization_inputs_result(cached.as_ref())
    }

    pub(crate) fn cached_shared_typed_module_plan(
        &self,
        key: SharedTypedModulePlanCacheKey,
        build: impl FnOnce() -> SharedTypedModulePlanResult,
    ) -> SharedTypedModulePlanResult {
        if let Some(cached) = self
            .shared_typed_module_plans
            .lock()
            .map_err(|_| {
                "compile session shared typed module plan cache lock poisoned".to_string()
            })?
            .get(&key)
            .cloned()
        {
            return clone_shared_typed_module_plan_result(cached.as_ref());
        }

        let built = Arc::new(build());
        let cached = {
            let mut cache = self.shared_typed_module_plans.lock().map_err(|_| {
                "compile session shared typed module plan cache lock poisoned".to_string()
            })?;
            cache
                .entry(key)
                .or_insert_with(|| Arc::clone(&built))
                .clone()
        };
        clone_shared_typed_module_plan_result(cached.as_ref())
    }

    pub fn retain_shared_module_state_for_inspection(
        &self,
        shared_state: Arc<SharedModuleState>,
    ) -> Result<(), String> {
        self.retain_shared_module_state(shared_state)
    }

    pub fn shared_module_state_for_function_id(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<Arc<SharedModuleState>>, String> {
        Ok(self
            .shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .for_function_id(function_id))
    }

    pub fn shared_module_state_for_identity(
        &self,
        module_name: &str,
        source_hash: u64,
    ) -> Result<Option<Arc<SharedModuleState>>, String> {
        Ok(self
            .shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .for_module_identity(module_name, source_hash))
    }

    pub(crate) fn shared_module_states_snapshot(
        &self,
    ) -> Result<Vec<Arc<SharedModuleState>>, String> {
        Ok(self
            .shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .retained
            .clone())
    }

    pub fn lookup_shared_function(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<(Arc<SharedModuleState>, BlockPyFunction<BlockPyModuleShape>)>, String> {
        if function_id == RuntimeFunctionId::global() {
            return Ok(None);
        }
        let Some(shared_state) = self.shared_module_state_for_function_id(function_id)? else {
            return Ok(None);
        };
        let Some(function) = shared_state.lookup_function(function_id).cloned() else {
            return Ok(None);
        };
        Ok(Some((shared_state, function)))
    }

    #[cfg(test)]
    fn retained_shared_module_state_count(&self) -> Result<usize, String> {
        Ok(self
            .shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .retained_len())
    }
}

fn clone_planned_optimization_inputs_result(
    result: &PlannedOptimizationInputsResult,
) -> PlannedOptimizationInputsResult {
    match result {
        Ok(inputs) => Ok(inputs.clone()),
        Err(err) => Err(err.clone()),
    }
}

fn clone_shared_typed_module_plan_result(
    result: &SharedTypedModulePlanResult,
) -> SharedTypedModulePlanResult {
    match result {
        Ok(plan) => Ok(Arc::clone(plan)),
        Err(err) => Err(err.clone()),
    }
}

impl Default for CompileSession {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CompileSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompileSession")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod test {
    use super::{
        CachedSharedTypedModulePlanMode, CompileSession, PlannedOptimizationInputsCacheKey,
        SharedTypedModulePlanCacheKey, allocate_compile_session_id,
    };
    use soac_config::SpecializationMode;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static SESSION_ID_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn profile_mode_caches_shared_typed_module_plans_without_counter_evidence() {
        let key = SharedTypedModulePlanCacheKey::new(
            1,
            2,
            None,
            SpecializationMode::Profile,
            false,
            false,
            false,
        )
        .expect("profile mode should cache a shared typed module plan without counter evidence");

        assert_eq!(
            key.specialization_mode,
            CachedSharedTypedModulePlanMode::Profile
        );
        assert!(key.counter_dump_path.is_none());
    }

    #[test]
    fn profile_mode_does_not_cache_planned_optimization_inputs() {
        let key = PlannedOptimizationInputsCacheKey::new(
            1,
            2,
            PathBuf::from("profile.bin"),
            SpecializationMode::Profile,
        );

        assert!(
            key.is_none(),
            "profile mode must not cache replay-only planned optimization inputs",
        );
    }

    #[test]
    fn replay_modes_require_counter_evidence_for_shared_typed_module_plans() {
        for mode in [SpecializationMode::Verify, SpecializationMode::Apply] {
            let key = SharedTypedModulePlanCacheKey::new(1, 2, None, mode, false, false, false);

            assert!(
                key.is_none(),
                "{mode:?} must not cache a shared typed module plan without counter evidence",
            );
        }
    }

    #[test]
    fn replay_modes_have_distinct_shared_typed_module_plan_cache_keys() {
        let counter_dump_path = PathBuf::from("profile.bin");
        let verify = SharedTypedModulePlanCacheKey::new(
            1,
            2,
            Some(counter_dump_path.clone()),
            SpecializationMode::Verify,
            false,
            false,
            false,
        )
        .expect("verify mode should cache a shared typed module plan with counter evidence");
        let apply = SharedTypedModulePlanCacheKey::new(
            1,
            2,
            Some(counter_dump_path),
            SpecializationMode::Apply,
            false,
            false,
            false,
        )
        .expect("apply mode should cache a shared typed module plan with counter evidence");

        assert_ne!(
            verify, apply,
            "verify and apply must not reuse each other's shared typed module plans",
        );
    }

    #[test]
    fn replay_modes_have_distinct_planned_optimization_input_cache_keys() {
        let counter_dump_path = PathBuf::from("profile.bin");
        let verify = PlannedOptimizationInputsCacheKey::new(
            1,
            2,
            counter_dump_path.clone(),
            SpecializationMode::Verify,
        )
        .expect("verify mode should cache replayed optimization inputs");
        let apply = PlannedOptimizationInputsCacheKey::new(
            1,
            2,
            counter_dump_path,
            SpecializationMode::Apply,
        )
        .expect("apply mode should cache replayed optimization inputs");

        assert_ne!(
            verify, apply,
            "verify and apply must not reuse each other's replayed optimization inputs",
        );
    }

    #[test]
    fn allocated_session_ids_increase_sequentially() {
        let _guard = SESSION_ID_TEST_LOCK.lock().unwrap();
        let first = allocate_compile_session_id();
        let second = allocate_compile_session_id();

        assert_eq!(second.as_u32(), first.as_u32() + 1);
    }

    #[test]
    fn compile_session_new_allocates_a_fresh_id() {
        let _guard = SESSION_ID_TEST_LOCK.lock().unwrap();
        let previous = allocate_compile_session_id();
        let session = CompileSession::new();

        assert_eq!(session.id().as_u32(), previous.as_u32() + 1);
    }

    #[test]
    fn compile_session_allocates_fresh_module_ids() {
        let session = CompileSession::new();
        let first = session.module_name_gen();
        let second = session.module_name_gen();

        assert_eq!(second.module_id(), first.module_id() + 1);
    }

    #[test]
    fn compile_session_starts_with_empty_shared_module_registry() {
        let session = CompileSession::new();

        assert_eq!(session.retained_shared_module_state_count().unwrap(), 0);
        assert!(
            session
                .shared_module_state_for_function_id(
                    soac_core::block_py::RuntimeFunctionId::from_raw_parts(7, 1)
                )
                .unwrap()
                .is_none()
        );
    }
}
