use soac_blockpy::block_py::ModuleNameGen;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_COMPILE_SESSION_ID: AtomicU32 = AtomicU32::new(1);

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CompileSession {
    id: CompileSessionId,
}

impl CompileSession {
    pub fn new() -> Self {
        Self {
            id: allocate_compile_session_id(),
        }
    }

    pub fn id(self) -> CompileSessionId {
        self.id
    }

    pub fn module_name_gen(self) -> ModuleNameGen {
        ModuleNameGen::new(self.id.as_u32())
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
}
