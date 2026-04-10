use crate::jit::ProcessJitEngine;
use crate::module_type::SharedModuleState;
use soac_blockpy::block_py::{BlockPyFunction, FunctionId, ModuleNameGen};
use soac_blockpy::passes::CodegenModuleShape;
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
}

#[derive(Default)]
struct SharedModuleStateRegistry {
    retained: Vec<Arc<SharedModuleState>>,
    by_module_id: HashMap<u32, usize>,
}

impl SharedModuleStateRegistry {
    fn retain(&mut self, shared_state: Arc<SharedModuleState>) {
        let module_id = shared_state.lowered_module.module_name_gen.module_id();
        let index = self.retained.len();
        self.retained.push(shared_state);
        self.by_module_id.insert(module_id, index);
    }

    fn for_function_id(&self, function_id: FunctionId) -> Option<Arc<SharedModuleState>> {
        let index = self.by_module_id.get(&function_id.module_id()).copied()?;
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

    pub(crate) fn process_jit(&self) -> Result<&ProcessJitEngine, String> {
        match self.process_jit.get_or_init(ProcessJitEngine::new) {
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

    pub(crate) fn shared_module_state_for_function_id(
        &self,
        function_id: FunctionId,
    ) -> Result<Option<Arc<SharedModuleState>>, String> {
        Ok(self
            .shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .for_function_id(function_id))
    }

    pub(crate) fn lookup_shared_function(
        &self,
        function_id: FunctionId,
    ) -> Result<Option<(Arc<SharedModuleState>, BlockPyFunction<CodegenModuleShape>)>, String> {
        if function_id == FunctionId::global() {
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
                .shared_module_state_for_function_id(soac_blockpy::block_py::FunctionId::new(7, 1))
                .unwrap()
                .is_none()
        );
    }
}
