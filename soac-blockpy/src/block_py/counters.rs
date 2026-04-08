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
        function_id: FunctionId,
        block_label: BlockLabel,
    },
    Runtime {
        function_id: Option<FunctionId>,
        instr_id: Option<InstrId>,
    },
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
