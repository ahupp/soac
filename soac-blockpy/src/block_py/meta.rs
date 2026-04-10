use super::{BlockLabel, FunctionId};
use ruff_python_ast::{self as ast, HasNodeIndex};
use ruff_text_size::{Ranged, TextRange};
use std::fmt;

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
pub struct InstrId {
    block_label: BlockLabel,
    instr_index_in_block: u32,
}

impl InstrId {
    pub const fn new(block_label: BlockLabel, instr_index_in_block: u32) -> Self {
        Self {
            block_label,
            instr_index_in_block,
        }
    }

    pub const fn block_label(self) -> BlockLabel {
        self.block_label
    }

    pub const fn instr_index_in_block(self) -> u32 {
        self.instr_index_in_block
    }
}

impl fmt::Display for InstrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.block_label, self.instr_index_in_block)
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
pub struct InstrKey {
    pub function_id: FunctionId,
    pub instr_id: InstrId,
}

impl InstrKey {
    pub const fn new(function_id: FunctionId, instr_id: InstrId) -> Self {
        Self {
            function_id,
            instr_id,
        }
    }
}

impl fmt::Display for InstrKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.function_id, self.instr_id)
    }
}

#[derive(Debug, Clone)]
pub struct IdentifiedInstr<I> {
    instr_id: InstrId,
    op: I,
}

impl<I> IdentifiedInstr<I> {
    pub const fn new(instr_id: InstrId, op: I) -> Self {
        Self { instr_id, op }
    }

    pub const fn instr_id(&self) -> InstrId {
        self.instr_id
    }

    pub const fn op(&self) -> &I {
        &self.op
    }

    pub fn into_op(self) -> I {
        self.op
    }
}

impl<I> HasMeta for IdentifiedInstr<I>
where
    I: HasMeta,
{
    fn meta(&self) -> Meta {
        let mut meta = self.op.meta();
        meta.instr_id = Some(self.instr_id);
        meta
    }
}

impl<I> WithMeta for IdentifiedInstr<I>
where
    I: HasMeta + WithMeta,
{
    fn with_meta(self, mut meta: Meta) -> Self {
        let instr_id = meta.instr_id.unwrap_or(self.instr_id);
        meta.instr_id = Some(instr_id);
        Self {
            instr_id,
            op: self.op.with_meta(meta),
        }
    }
}

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Meta {
    #[rkyv(with = rkyv::with::Skip)]
    pub node_index: ast::AtomicNodeIndex,
    pub instr_id: Option<InstrId>,
    #[rkyv(with = rkyv::with::Skip)]
    pub range: TextRange,
}

impl Meta {
    pub fn new(node_index: ast::AtomicNodeIndex, range: TextRange) -> Self {
        Self {
            node_index,
            instr_id: None,
            range,
        }
    }

    pub fn synthetic() -> Self {
        Self::default()
    }
}

pub trait HasMeta {
    fn meta(&self) -> Meta;
}

pub trait WithMeta: Sized {
    fn with_meta(self, meta: Meta) -> Self;

    fn with_source<T: HasMeta>(self, source: &T) -> Self {
        self.with_meta(source.meta())
    }
}

pub trait HasSemanticInstrId: HasMeta {
    fn try_semantic_instr_id(&self) -> Option<InstrId> {
        self.meta().instr_id
    }

    fn semantic_instr_id(&self) -> InstrId {
        self.try_semantic_instr_id()
            .expect("semantic codegen instruction id should be assigned")
    }

    fn semantic_instr_key(&self, function_id: FunctionId) -> InstrKey {
        InstrKey::new(function_id, self.semantic_instr_id())
    }
}

impl<T> HasSemanticInstrId for T where T: HasMeta {}

impl<T> HasMeta for T
where
    T: HasNodeIndex + Ranged,
{
    fn meta(&self) -> Meta {
        Meta::new(self.node_index().clone(), self.range())
    }
}
