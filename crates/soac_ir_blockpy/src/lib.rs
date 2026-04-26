#![deny(unreachable_pub)]

mod constructor_entries;
mod instr_id;

pub(crate) mod block_py {
    pub(crate) use soac_core::block_py::*;
}

use crate::block_py::{
    BinOp, Block, BlockPyFormat, Call, CellRef, ChildVisitable, ConstantExpr, Del, DelItem,
    GetAttr, GetItem, HasMeta, IncrementCounter, Instr, InstrWithConstantNone, Load, MakeCell,
    MakeFunctionWithClosure, MapInstr, Mappable, Meta, ModuleShape, NameLike, ResolvedName,
    SetAttr, SetItem, Store, TryMapInstr, Tuple, UnaryOp, WithMeta,
};
use soac_macros::{DelegateMatchDefault, enum_broadcast};

pub use crate::instr_id::{
    assign_codegen_module_instr_ids, assign_missing_codegen_function_instr_ids,
    reassign_codegen_function_instr_ids, reassign_codegen_module_instr_ids,
    validate_codegen_instr_ids,
};
pub use constructor_entries::{
    CONSTRUCTOR_ENTRY_FUNCTION_NAME, constructor_entry_function_id_for_init,
    ensure_constructor_entry_functions,
};

/// Final lowered BlockPy form consumed by optimization, instrumentation, and JIT.
///
/// Compared with resolved-storage BlockPy, module constants have been hoisted
/// into the module constant table and codegen-facing operations such as
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
pub enum InstrCodegen {
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
    MakeCell(#[rkyv(omit_bounds)] MakeCell<Self>),
    IncrementCounter(IncrementCounter),
    CellRef(CellRef),
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
}

impl Instr for InstrCodegen {
    type Name = ResolvedName;
    type Extra = ();
}

impl InstrWithConstantNone for InstrCodegen {
    fn constant_none() -> Self {
        Load::new(ResolvedName::runtime_name("NONE")).into()
    }
}

/// Module shape for final codegen-ready BlockPy.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenModuleShape;

impl ModuleShape for CodegenModuleShape {
    type Instr = InstrCodegen;
    type ModuleConstant = ConstantExpr;
    type BlockExtra = ();
}

impl BlockPyFormat for CodegenModuleShape {
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
