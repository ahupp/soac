use super::operation_macro::define_operation;
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
}

define_operation! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}
