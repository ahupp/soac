use super::BlockLabel;
use ruff_python_ast::{self as ast, HasNodeIndex};
use ruff_text_size::{Ranged, TextRange};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub node_index: ast::AtomicNodeIndex,
    pub instr_id: Option<InstrId>,
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

impl<T> HasMeta for T
where
    T: HasNodeIndex + Ranged,
{
    fn meta(&self) -> Meta {
        Meta::new(self.node_index().clone(), self.range())
    }
}
