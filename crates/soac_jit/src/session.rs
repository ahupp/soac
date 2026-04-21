use crate::jit::ProcessJitEngine;
use crate::module_type::SharedModuleState;
use soac_config::SoacEnvConfig;
use soac_core::block_py::{BlockPyFunction, ModuleNameGen, RuntimeFunctionId};
use soac_lowering::passes::CodegenModuleShape;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
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
    shared_module_states: Mutex<SharedModuleStateRegistry>,
    process_jit: OnceLock<Result<ProcessJitEngine, String>>,
    env_config: OnceLock<Result<SoacEnvConfig, String>>,
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
            shared_module_states: Mutex::new(SharedModuleStateRegistry::default()),
            process_jit: OnceLock::new(),
            env_config: OnceLock::new(),
        }
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
        Ok(())
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

    pub fn lookup_shared_function(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Result<Option<(Arc<SharedModuleState>, BlockPyFunction<CodegenModuleShape>)>, String> {
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
    use super::{CompileSession, allocate_compile_session_id};
    use std::sync::Mutex;

    static SESSION_ID_TEST_LOCK: Mutex<()> = Mutex::new(());

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
