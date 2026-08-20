#![deny(unreachable_pub)]

use soac_core::block_py::{BuildCollection, CallArgumentOp, IteratorStep, PreparedCall};
mod constructor_entries;
mod instr_id;

pub(crate) mod block_py {
    pub(crate) use soac_core::block_py::*;
}

use crate::block_py::{
    ApplyClassDecorator, ApplyFunctionDescriptor, BinOp, Block, BlockPyFormat, Call, CellRef,
    CheckAnnotationFormat, ChildVisitable, CompleteFunctionDefinition, ComprehensionInsert,
    ConstantExpr, ConstructClass, ConstructTypeParameterScope, CreateTypeAlias,
    CreateTypeParameter, Del, DelItem, DiscardClassConstructionCaptures, DiscardClassDecorator,
    GetAttr, GetItem, HasMeta, IncrementCounter, Instr, InstrWithConstantNone, Load, MakeCell,
    MakeFunctionWithClosure, MapInstr, Mappable, Meta, ModuleShape, NameLike, NewAnnotationSet,
    PrepareClassDecorator, RecordAnnotation, ResolvedName, SetAttr, SetFunctionTypeParameters,
    SetItem, SetTypeParameterDefault, SetupAnnotations, Store, SubscriptGeneric, TakeOperand,
    TryMapInstr, Tuple, UnaryOp, WithMeta,
};
use soac_macros::{DelegateMatchDefault, enum_broadcast};

pub use crate::instr_id::{
    assign_blockpy_module_instr_ids, assign_missing_blockpy_function_instr_ids,
    reassign_blockpy_function_instr_ids, reassign_blockpy_module_instr_ids,
    validate_blockpy_instr_ids,
};
pub use constructor_entries::{
    CONSTRUCTOR_ENTRY_FUNCTION_NAME, CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME,
    constructor_entry_function_id_for_init, constructor_init_function_id_for_entry_function,
    ensure_constructor_entry_functions, is_constructor_entry_function,
};

/// Final lowered BlockPy form consumed by optimization, instrumentation, and JIT.
///
/// Compared with resolved-storage BlockPy, module constants have been hoisted
/// into the module constant table and BlockPy-only operations such as
/// explicit counter increments can appear.
#[derive(
    Clone,
    derive_more::From,
    DelegateMatchDefault,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(serialize_bounds(
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(bounds(
    __C: rkyv::validation::ArchiveContext,
)))]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub enum InstrBlockPy {
    BinOp(#[rkyv(omit_bounds)] BinOp<Self>),
    UnaryOp(#[rkyv(omit_bounds)] UnaryOp<Self>),
    Tuple(#[rkyv(omit_bounds)] Tuple<Self>),
    Call(#[rkyv(omit_bounds)] Call<Self>),
    GetAttr(#[rkyv(omit_bounds)] GetAttr<Self>),
    SetAttr(#[rkyv(omit_bounds)] SetAttr<Self>),
    GetItem(#[rkyv(omit_bounds)] GetItem<Self>),
    SetItem(#[rkyv(omit_bounds)] SetItem<Self>),
    DelItem(#[rkyv(omit_bounds)] DelItem<Self>),
    Load(#[rkyv(omit_bounds)] Load<Self>),
    Store(#[rkyv(omit_bounds)] Store<Self>),
    Del(#[rkyv(omit_bounds)] Del<Self>),
    TakeOperand(#[rkyv(omit_bounds)] TakeOperand<Self>),
    ComprehensionInsert(#[rkyv(omit_bounds)] ComprehensionInsert<Self>),
    BuildCollection(#[rkyv(omit_bounds)] BuildCollection<Self>),
    CallArgumentOp(#[rkyv(omit_bounds)] CallArgumentOp<Self>),
    PreparedCall(#[rkyv(omit_bounds)] PreparedCall<Self>),
    IteratorStep(#[rkyv(omit_bounds)] IteratorStep<Self>),
    MakeCell(#[rkyv(omit_bounds)] MakeCell<Self>),
    NewAnnotationSet(#[rkyv(omit_bounds)] NewAnnotationSet<Self>),
    SetupAnnotations(#[rkyv(omit_bounds)] SetupAnnotations<Self>),
    ConstructTypeParameterScope(#[rkyv(omit_bounds)] ConstructTypeParameterScope<Self>),
    SubscriptGeneric(#[rkyv(omit_bounds)] SubscriptGeneric<Self>),
    SetFunctionTypeParameters(#[rkyv(omit_bounds)] SetFunctionTypeParameters<Self>),
    CreateTypeAlias(#[rkyv(omit_bounds)] CreateTypeAlias<Self>),
    CreateTypeParameter(#[rkyv(omit_bounds)] CreateTypeParameter<Self>),
    SetTypeParameterDefault(#[rkyv(omit_bounds)] SetTypeParameterDefault<Self>),
    CheckAnnotationFormat(#[rkyv(omit_bounds)] CheckAnnotationFormat<Self>),
    RecordAnnotation(#[rkyv(omit_bounds)] RecordAnnotation<Self>),
    IncrementCounter(IncrementCounter),
    CellRef(CellRef),
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
    CompleteFunctionDefinition(#[rkyv(omit_bounds)] CompleteFunctionDefinition<Self>),
    ApplyFunctionDescriptor(#[rkyv(omit_bounds)] ApplyFunctionDescriptor<Self>),
    PrepareClassDecorator(#[rkyv(omit_bounds)] PrepareClassDecorator<Self>),
    ApplyClassDecorator(#[rkyv(omit_bounds)] ApplyClassDecorator<Self>),
    DiscardClassDecorator(#[rkyv(omit_bounds)] DiscardClassDecorator<Self>),
    DiscardClassConstructionCaptures(#[rkyv(omit_bounds)] DiscardClassConstructionCaptures<Self>),
    ConstructClass(#[rkyv(omit_bounds)] ConstructClass<Self>),
}

impl soac_core::block_py::TakeOperandInstruction for InstrBlockPy {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
        match self {
            Self::TakeOperand(op) => Some(op),
            _ => None,
        }
    }
}

impl Instr for InstrBlockPy {
    type Name = ResolvedName;
    type Extra = ();
}

impl InstrWithConstantNone for InstrBlockPy {
    fn constant_none() -> Self {
        Load::new(ResolvedName::runtime_name("NONE")).into()
    }
}

/// Module shape for final pre-typed BlockPy.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BlockPyModuleShape;

impl ModuleShape for BlockPyModuleShape {
    type Instr = InstrBlockPy;
    type ModuleConstant = ConstantExpr;
    type BlockExtra = soac_core::block_py::BlockContext;
}

impl BlockPyFormat for BlockPyModuleShape {
    fn block_metadata_lines(block: &Block<Self::Instr, Self::BlockExtra>) -> Vec<String> {
        let mut lines = Vec::new();
        if !block.params.is_empty() {
            lines.push(format!(
                "params: [{}]",
                block
                    .param_names()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(exc_edge) = &block.exc_edge {
            lines.push(format!("exc_target: {}", exc_edge.target));
        }
        if let Some(exc_name) = block.exception_param() {
            lines.push(format!("exc_name: {exc_name}"));
        }
        lines
    }
}
