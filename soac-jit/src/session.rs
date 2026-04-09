use crate::jit::ProcessJitEngine;
use crate::module_type::SharedModuleState;
use soac_blockpy::block_py::ModuleNameGen;
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
    shared_module_states: Mutex<Vec<Arc<SharedModuleState>>>,
    process_jit: OnceLock<Result<ProcessJitEngine, String>>,
}

impl CompileSession {
    pub fn new() -> Self {
        Self {
            id: allocate_compile_session_id(),
            next_module_id: AtomicU32::new(1),
            shared_module_states: Mutex::new(Vec::new()),
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
            .push(shared_state);
        Ok(())
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
}
