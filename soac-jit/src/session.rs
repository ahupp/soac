use crate::jit::ProcessJitEngine;
use crate::module_type::SharedModuleState;
use cranelift_codegen::incremental_cache::CacheKvStore;
use soac_blockpy::block_py::{BlockPyFunction, FunctionId, ModuleNameGen};
use soac_blockpy::passes::CodegenModuleShape;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
    cranelift_compile_cache: CraneliftCompileCache,
}

#[derive(Debug)]
pub(crate) struct CraneliftCompileCache {
    enabled: bool,
    root: PathBuf,
}

pub(crate) struct CraneliftCompileCacheStore<'a> {
    cache: &'a CraneliftCompileCache,
}

impl CraneliftCompileCache {
    fn new(root: impl Into<PathBuf>, enabled: bool) -> Self {
        Self {
            enabled,
            root: root.into(),
        }
    }

    fn from_env() -> Self {
        Self::new(
            CraneliftCompileCache::default_root(),
            parse_cranelift_compile_cache_enabled(
                std::env::var("SOAC_CRANELIFT_COMPILE_CACHE")
                    .ok()
                    .as_deref(),
            ),
        )
    }

    fn default_root() -> PathBuf {
        find_repo_root_for_compile_cache()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cl-cache")
    }

    pub(crate) fn store(&self) -> CraneliftCompileCacheStore<'_> {
        CraneliftCompileCacheStore { cache: self }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn path_for_key(&self, key: &[u8]) -> PathBuf {
        self.root.join(hex_cache_key(key))
    }

    fn temp_path_for_key(&self, key: &[u8]) -> PathBuf {
        let name = hex_cache_key(key);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        self.root
            .join(format!(".{name}.{}.{}.tmp", std::process::id(), timestamp))
    }

    fn read(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        fs::read(self.path_for_key(key)).ok().map(Cow::Owned)
    }

    fn write(&self, key: &[u8], value: Vec<u8>) -> std::io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let temp_path = self.temp_path_for_key(key);
        fs::write(&temp_path, value)?;
        if let Err(err) = fs::rename(&temp_path, self.path_for_key(key)) {
            let _ = fs::remove_file(&temp_path);
            return Err(err);
        }
        Ok(())
    }
}

fn parse_cranelift_compile_cache_enabled(raw: Option<&str>) -> bool {
    raw.map(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
    .unwrap_or(false)
}

impl CacheKvStore for CraneliftCompileCacheStore<'_> {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        self.cache.read(key)
    }

    fn insert(&mut self, key: &[u8], val: Vec<u8>) {
        if let Err(err) = self.cache.write(key, val) {
            tracing::debug!(
                target: "soac_jit_compile_cache",
                cache_root = %self.cache.root.display(),
                error = %err,
                "failed to store Cranelift compile cache entry"
            );
        }
    }
}

fn hex_cache_key(key: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(key.len() * 2);
    for byte in key {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn find_repo_root_for_compile_cache() -> Option<PathBuf> {
    let start = std::env::current_dir().ok()?;
    for candidate in start.ancestors() {
        if candidate.join("Justfile").is_file() && candidate.join("soac-jit").is_dir() {
            return Some(candidate.to_path_buf());
        }
    }
    Some(start)
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
            cranelift_compile_cache: CraneliftCompileCache::from_env(),
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
        match self.process_jit.get_or_init(|| ProcessJitEngine::new(self)) {
            Ok(engine) => Ok(engine),
            Err(err) => Err(err.clone()),
        }
    }

    pub(crate) fn cranelift_compile_cache(&self) -> &CraneliftCompileCache {
        &self.cranelift_compile_cache
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

    pub fn shared_module_state_for_function_id(
        &self,
        function_id: FunctionId,
    ) -> Result<Option<Arc<SharedModuleState>>, String> {
        Ok(self
            .shared_module_states
            .lock()
            .map_err(|_| "compile session shared module state lock poisoned".to_string())?
            .for_function_id(function_id))
    }

    pub fn lookup_shared_function(
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
    use super::{
        CompileSession, CraneliftCompileCache, allocate_compile_session_id,
        parse_cranelift_compile_cache_enabled,
    };
    use cranelift_codegen::incremental_cache::CacheKvStore;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn cranelift_compile_cache_writes_values_to_key_named_paths() {
        let root = unique_temp_dir("soac-cl-cache-test");
        let cache = CraneliftCompileCache::new(&root, true);
        let mut store = cache.store();
        let key = [0x00, 0x01, 0xab, 0xff];
        let value = b"compiled-value".to_vec();

        store.insert(&key, value.clone());

        let expected_path = root.join("0001abff");
        assert_eq!(fs::read(&expected_path).unwrap(), value);
        assert_eq!(store.get(&key).unwrap().as_ref(), b"compiled-value");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compile_session_carries_cranelift_compile_cache_root() {
        let session = CompileSession::new();

        let path = session
            .cranelift_compile_cache()
            .path_for_key(&[0xab, 0xcd]);
        assert!(path.ends_with(Path::new(".cl-cache").join("abcd")));
    }

    #[test]
    fn cranelift_compile_cache_is_opt_in() {
        assert!(!parse_cranelift_compile_cache_enabled(None));
        assert!(!parse_cranelift_compile_cache_enabled(Some("")));
        assert!(!parse_cranelift_compile_cache_enabled(Some("0")));
        assert!(parse_cranelift_compile_cache_enabled(Some("1")));
        assert!(parse_cranelift_compile_cache_enabled(Some("true")));
        assert!(parse_cranelift_compile_cache_enabled(Some("ON")));
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("{prefix}-{}-{timestamp}", std::process::id()))
    }
}
