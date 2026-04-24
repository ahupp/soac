use super::instr_macro::define_instr;
use super::*;

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
pub struct CounterId(pub usize);

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
pub struct CounterBranchId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CounterBranch {
    pub name: String,
}

impl CounterBranch {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum CounterScope {
    This,
    Function,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CounterSite {
    BlockEntry {
        function_id: RuntimeFunctionId,
        block_label: BlockLabel,
    },
    DeoptEntry {
        function_id: RuntimeFunctionId,
        source: DeoptEntrySource,
    },
    // `instr_id` names the semantic instruction site being observed. Synthetic
    // instrumentation instructions may have no semantic id of their own.
    Runtime {
        function_id: Option<RuntimeFunctionId>,
        instr_id: Option<InstrId>,
    },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum DeoptEntrySource {
    BlockEntry { block_label: BlockLabel },
    BeforeInstr { instr_id: InstrId },
    BeforeTerm { block_label: BlockLabel },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CounterDef {
    pub id: CounterId,
    pub scope: CounterScope,
    pub kind: String,
    pub site: CounterSite,
    pub branches: Vec<CounterBranch>,
}

impl CounterDef {
    pub fn scalar(
        id: CounterId,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
    ) -> Self {
        Self {
            id,
            scope,
            kind: kind.into(),
            site,
            branches: Vec::new(),
        }
    }

    pub fn branch_counter(
        id: CounterId,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
        branches: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id,
            scope,
            kind: kind.into(),
            site,
            branches: branches.into_iter().map(CounterBranch::new).collect(),
        }
    }

    pub fn branch_id(&self, name: &str) -> Option<CounterBranchId> {
        self.branches
            .iter()
            .position(|branch| branch.name == name)
            .map(CounterBranchId)
    }

    pub fn branch_name(&self, branch_id: CounterBranchId) -> Option<&str> {
        self.branches
            .get(branch_id.0)
            .map(|branch| branch.name.as_str())
    }

    pub fn is_branch_counter(&self) -> bool {
        !self.branches.is_empty()
    }
}

define_instr! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}
