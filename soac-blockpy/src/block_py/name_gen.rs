use ruff_python_ast as ast;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct LocalFunctionId(u32);

impl LocalFunctionId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for LocalFunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct RuntimeModuleId(u32);

impl RuntimeModuleId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for RuntimeModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct SerializedModuleId(u32);

impl SerializedModuleId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SerializedModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct RuntimeFunctionId(u64);

impl RuntimeFunctionId {
    pub const GLOBAL: Self = Self(u64::MAX);

    pub const fn new(module_id: RuntimeModuleId, function_id: LocalFunctionId) -> Self {
        Self(((module_id.as_u32() as u64) << 32) | function_id.as_u32() as u64)
    }

    pub const fn from_packed(packed: u64) -> Self {
        Self(packed)
    }

    pub const fn global() -> Self {
        Self::GLOBAL
    }

    pub const fn packed(self) -> u64 {
        self.0
    }

    pub const fn module_id(self) -> RuntimeModuleId {
        RuntimeModuleId((self.0 >> 32) as u32)
    }

    pub const fn local_function_id(self) -> LocalFunctionId {
        LocalFunctionId(self.0 as u32)
    }
}

impl fmt::Debug for RuntimeFunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module_id(), self.local_function_id())
    }
}

impl fmt::Display for RuntimeFunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module_id(), self.local_function_id())
    }
}

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct SerializedFunctionId(u64);

impl SerializedFunctionId {
    pub const fn new(module_id: SerializedModuleId, function_id: LocalFunctionId) -> Self {
        Self(((module_id.as_u32() as u64) << 32) | function_id.as_u32() as u64)
    }

    pub const fn from_packed(packed: u64) -> Self {
        Self(packed)
    }

    pub const fn packed(self) -> u64 {
        self.0
    }

    pub const fn module_id(self) -> SerializedModuleId {
        SerializedModuleId((self.0 >> 32) as u32)
    }

    pub const fn local_function_id(self) -> LocalFunctionId {
        LocalFunctionId(self.0 as u32)
    }
}

impl fmt::Debug for SerializedFunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module_id(), self.local_function_id())
    }
}

impl fmt::Display for SerializedFunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module_id(), self.local_function_id())
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct ModuleContentId {
    pub module_name: String,
    pub source_hash: u64,
}

impl ModuleContentId {
    pub fn new(module_name: impl Into<String>, source_hash: u64) -> Self {
        Self {
            module_name: module_name.into(),
            source_hash,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct PersistentFunctionId {
    pub module: ModuleContentId,
    pub local: LocalFunctionId,
}

impl PersistentFunctionId {
    pub fn new(module: ModuleContentId, local: LocalFunctionId) -> Self {
        Self { module, local }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct SerializedModuleIdentity {
    pub module_name: String,
    pub source_hash: u64,
    pub cache_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct SerializedFunctionDebugName {
    pub function: SerializedFunctionId,
    pub qualname: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct SerializedIdentityTables {
    pub modules: Vec<SerializedModuleIdentity>,
    pub debug_names: Vec<SerializedFunctionDebugName>,
}

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct FunctionId(u64);

impl FunctionId {
    pub const GLOBAL: Self = Self(u64::MAX);

    pub const fn new(module_id: u32, function_id: u32) -> Self {
        Self(((module_id as u64) << 32) | function_id as u64)
    }

    pub const fn from_packed(packed: u64) -> Self {
        Self(packed)
    }

    pub const fn from_runtime(runtime_id: RuntimeFunctionId) -> Self {
        Self(runtime_id.packed())
    }

    pub const fn from_runtime_parts(
        module_id: RuntimeModuleId,
        function_id: LocalFunctionId,
    ) -> Self {
        Self::from_runtime(RuntimeFunctionId::new(module_id, function_id))
    }

    pub const fn global() -> Self {
        Self::GLOBAL
    }

    pub const fn packed(self) -> u64 {
        self.0
    }

    pub const fn module_id(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn function_id(self) -> u32 {
        self.0 as u32
    }

    pub const fn runtime_id(self) -> RuntimeFunctionId {
        RuntimeFunctionId::from_packed(self.0)
    }

    pub const fn runtime_module_id(self) -> RuntimeModuleId {
        self.runtime_id().module_id()
    }

    pub const fn local_function_id(self) -> LocalFunctionId {
        self.runtime_id().local_function_id()
    }
}

impl From<RuntimeFunctionId> for FunctionId {
    fn from(value: RuntimeFunctionId) -> Self {
        Self::from_runtime(value)
    }
}

impl From<FunctionId> for RuntimeFunctionId {
    fn from(value: FunctionId) -> Self {
        value.runtime_id()
    }
}

impl fmt::Debug for FunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module_id(), self.function_id())
    }
}

impl fmt::Display for FunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module_id(), self.function_id())
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct BlockLabel {
    index: u32,
}

impl BlockLabel {
    pub const FALLTHROUGH_INDEX: u32 = u32::MAX;

    pub fn from_index(value: usize) -> Self {
        Self {
            index: u32::try_from(value).expect("block label usize should fit in u32"),
        }
    }

    pub const fn fallthrough() -> Self {
        Self {
            index: Self::FALLTHROUGH_INDEX,
        }
    }

    pub const fn is_fallthrough(self) -> bool {
        self.index == Self::FALLTHROUGH_INDEX
    }

    pub const fn as_u32(self) -> u32 {
        self.index
    }

    pub fn index(self) -> usize {
        self.index as usize
    }
}

impl fmt::Display for BlockLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_fallthrough() {
            write!(f, "<fallthrough>")
        } else {
            write!(f, "bb{}", self.index)
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionNameGen {
    state: Arc<FunctionNameGenState>,
}

#[derive(Debug)]
struct FunctionNameGenState {
    function_id: FunctionId,
    next_block_id: AtomicUsize,
    next_tmp_id: AtomicUsize,
}

impl FunctionNameGen {
    fn new(function_id: FunctionId) -> Self {
        Self::recovered(function_id, 0, 0)
    }

    pub fn recovered(function_id: FunctionId, next_block_id: u32, next_tmp_id: usize) -> Self {
        Self {
            state: Arc::new(FunctionNameGenState {
                function_id,
                next_block_id: AtomicUsize::new(next_block_id as usize),
                next_tmp_id: AtomicUsize::new(next_tmp_id),
            }),
        }
    }

    pub(crate) fn share(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }

    pub fn function_id(&self) -> FunctionId {
        self.state.function_id
    }

    pub fn next_block_name(&self) -> BlockLabel {
        let current = self.state.next_block_id.fetch_add(1, Ordering::Relaxed);
        BlockLabel::from_index(current)
    }

    pub fn next_tmp_name(&self, prefix: &str) -> ast::name::Name {
        let current = self.state.next_tmp_id.fetch_add(1, Ordering::Relaxed);
        ast::name::Name::new(format!(
            "_dp_{prefix}_{}_{}_{}",
            self.state.function_id.module_id(),
            self.state.function_id.function_id(),
            current
        ))
    }
}

#[derive(Debug)]
pub struct ModuleNameGen {
    module_id: u32,
    state: Arc<AtomicU32>,
}

impl ModuleNameGen {
    pub fn new(module_id: u32) -> Self {
        Self::recovered(module_id, 1)
    }

    pub fn recovered(module_id: u32, next_function_id: u32) -> Self {
        Self {
            module_id,
            state: Arc::new(AtomicU32::new(next_function_id)),
        }
    }

    pub fn module_id(&self) -> u32 {
        self.module_id
    }

    pub fn next_function_name_gen(&self) -> FunctionNameGen {
        let function_id =
            FunctionId::new(self.module_id, self.state.fetch_add(1, Ordering::Relaxed));
        FunctionNameGen::new(function_id)
    }
}

impl Clone for ModuleNameGen {
    fn clone(&self) -> Self {
        Self {
            module_id: self.module_id,
            state: Arc::clone(&self.state),
        }
    }
}

impl Default for ModuleNameGen {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Default for FunctionNameGen {
    fn default() -> Self {
        Self::recovered(FunctionId::global(), 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FunctionId, LocalFunctionId, RuntimeFunctionId, RuntimeModuleId,
        SerializedFunctionDebugName, SerializedFunctionId, SerializedModuleId,
    };

    #[test]
    fn runtime_function_id_roundtrips_current_packed_function_id() {
        let runtime_id = RuntimeFunctionId::new(RuntimeModuleId::new(17), LocalFunctionId::new(42));
        let function_id = FunctionId::from_runtime(runtime_id);

        assert_eq!(function_id.packed(), runtime_id.packed());
        assert_eq!(function_id.runtime_module_id(), RuntimeModuleId::new(17));
        assert_eq!(function_id.local_function_id(), LocalFunctionId::new(42));
        assert_eq!(RuntimeFunctionId::from(function_id), runtime_id);
    }

    #[test]
    fn serialized_function_id_uses_serialized_module_index_not_runtime_module_id() {
        let serialized_id =
            SerializedFunctionId::new(SerializedModuleId::new(3), LocalFunctionId::new(42));

        assert_eq!(serialized_id.packed(), (3_u64 << 32) | 42);
        assert_eq!(serialized_id.module_id(), SerializedModuleId::new(3));
        assert_eq!(serialized_id.local_function_id(), LocalFunctionId::new(42));
    }

    #[test]
    fn serialized_function_debug_name_keeps_qualname_out_of_identity() {
        let function =
            SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(7));
        let debug_name = SerializedFunctionDebugName {
            function,
            qualname: "outer.<locals>.inner".to_string(),
        };

        assert_eq!(debug_name.function, function);
    }
}
