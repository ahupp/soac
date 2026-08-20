use super::{
    BlockLabel, BlockPyFunction, ChildVisitable, Instr, ModuleShape, RuntimeFunctionId, Visit,
};
use ruff_python_ast::{self as ast, HasNodeIndex};
use ruff_text_size::{Ranged, TextRange};
use std::collections::HashMap;
use std::fmt;

/// Source ranges are semantic anchors for authenticated offline operation
/// facts. Ruff's process-local node indexes are still omitted from archives,
/// but dropping ranges would silently disable those decisions on cache hits.
pub(super) struct ArchivedSourceRange;

impl rkyv::with::ArchiveWith<TextRange> for ArchivedSourceRange {
    type Archived = rkyv::Archived<[u32; 2]>;
    type Resolver = rkyv::Resolver<[u32; 2]>;

    fn resolve_with(field: &TextRange, resolver: Self::Resolver, out: rkyv::Place<Self::Archived>) {
        rkyv::Archive::resolve(
            &[field.start().to_u32(), field.end().to_u32()],
            resolver,
            out,
        );
    }
}

impl<S> rkyv::with::SerializeWith<TextRange, S> for ArchivedSourceRange
where
    S: rkyv::rancor::Fallible + ?Sized,
    [u32; 2]: rkyv::Serialize<S>,
{
    fn serialize_with(field: &TextRange, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        rkyv::Serialize::serialize(&[field.start().to_u32(), field.end().to_u32()], serializer)
    }
}

impl<D> rkyv::with::DeserializeWith<rkyv::Archived<[u32; 2]>, TextRange, D> for ArchivedSourceRange
where
    D: rkyv::rancor::Fallible + ?Sized,
    D::Error: rkyv::rancor::Source,
    rkyv::Archived<[u32; 2]>: rkyv::Deserialize<[u32; 2], D>,
{
    fn deserialize_with(
        field: &rkyv::Archived<[u32; 2]>,
        deserializer: &mut D,
    ) -> Result<TextRange, D::Error> {
        use rkyv::rancor::Source;
        let [start, end] = rkyv::Deserialize::deserialize(field, deserializer)?;
        if start > end {
            return Err(D::Error::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archived source range starts after its end",
            )));
        }
        Ok(TextRange::new(start.into(), end.into()))
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
pub struct InstrId {
    index: u32,
}

impl InstrId {
    pub const fn new(index: u32) -> Self {
        Self { index }
    }

    pub const fn index(self) -> u32 {
        self.index
    }
}

impl fmt::Display for InstrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrLocation {
    block_label: BlockLabel,
    body_index: Option<usize>,
}

impl InstrLocation {
    pub const fn new(block_label: BlockLabel, body_index: Option<usize>) -> Self {
        Self {
            block_label,
            body_index,
        }
    }

    pub const fn block_label(self) -> BlockLabel {
        self.block_label
    }

    pub const fn body_index(self) -> Option<usize> {
        self.body_index
    }
}

pub type InstrLocationMap = HashMap<InstrId, InstrLocation>;

struct InstrLocationCollector<'a> {
    locations: &'a mut InstrLocationMap,
    block_label: BlockLabel,
    body_index: Option<usize>,
}

impl<I> super::Visit<I> for InstrLocationCollector<'_>
where
    I: Instr + ChildVisitable<I> + HasMeta,
{
    fn visit_instr(&mut self, expr: &I)
    where
        I: ChildVisitable<I>,
    {
        if let Some(instr_id) = expr.try_semantic_instr_id() {
            self.locations
                .entry(instr_id)
                .or_insert_with(|| InstrLocation::new(self.block_label, self.body_index));
        }
        expr.visit_children(self);
    }
}

pub fn current_instr_locations<P>(function: &BlockPyFunction<P>) -> InstrLocationMap
where
    P: ModuleShape,
    P::Instr: ChildVisitable<P::Instr> + HasMeta,
{
    let mut locations = HashMap::new();
    for block in &function.blocks {
        for (body_index, instr) in block.body.iter().enumerate() {
            let mut collector = InstrLocationCollector {
                locations: &mut locations,
                block_label: block.label,
                body_index: Some(body_index),
            };
            collector.visit_instr(instr);
        }
        let mut collector = InstrLocationCollector {
            locations: &mut locations,
            block_label: block.label,
            body_index: None,
        };
        collector.visit_term(&block.term);
    }
    locations
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
pub struct InstrKey {
    pub function_id: RuntimeFunctionId,
    pub instr_id: InstrId,
}

impl InstrKey {
    pub const fn new(function_id: RuntimeFunctionId, instr_id: InstrId) -> Self {
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

impl<I> Instr for IdentifiedInstr<I>
where
    I: Instr,
{
    type Name = I::Name;
    type Extra = I::Extra;
}

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Meta {
    #[rkyv(with = rkyv::with::Skip)]
    pub node_index: ast::AtomicNodeIndex,
    pub instr_id: Option<InstrId>,
    #[rkyv(with = ArchivedSourceRange)]
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

    fn semantic_instr_key(&self, function_id: RuntimeFunctionId) -> InstrKey {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archived_metadata_preserves_authenticated_source_site_ranges() {
        let meta = Meta {
            instr_id: Some(InstrId::new(17)),
            range: TextRange::new(23.into(), 51.into()),
            ..Meta::default()
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&meta).unwrap();
        let restored = rkyv::from_bytes::<Meta, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(restored.instr_id, meta.instr_id);
        assert_eq!(restored.range, meta.range);
    }
}
