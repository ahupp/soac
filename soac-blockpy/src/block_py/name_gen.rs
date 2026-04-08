use ruff_python_ast as ast;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

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
pub struct FunctionId(u64);

impl FunctionId {
    pub const GLOBAL: Self = Self(u64::MAX);

    pub const fn new(module_id: u32, function_id: u32) -> Self {
        Self(((module_id as u64) << 32) | function_id as u64)
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

    pub const fn module_id(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn function_id(self) -> u32 {
        self.0 as u32
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
