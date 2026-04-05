pub(crate) mod ast_symbol_analysis;
pub(crate) mod ast_to_ast;
pub(crate) mod blockpy_expr_simplify;
mod blockpy_generators;
pub mod blockpy_to_bb;
pub(crate) mod core_await_lower;
mod instrument;
mod instr_id;
mod name_binding;
pub mod ruff_to_blockpy;
mod trace;

use crate::block_py::{cfg::relabel_blockpy_blocks_dense, BlockPyModule};
use crate::block_py::{
    Await, BinOp, BlockPyNameLike, BlockPyPass, Call, CellRef, CellRefForName, ChildVisitable,
    CodegenBlockPyExpr, Del, DelItem, GetAttr, GetItem, HasMeta, Instr, LiteralValue, Load,
    LocatedName, MakeCell, MakeFunction, MapInstr, Mappable, Meta, SetAttr, SetItem, Store,
    TryMapInstr, UnaryOp, UnresolvedName, WithMeta, Yield, YieldFrom,
};
use soac_macros::{enum_broadcast, DelegateMatchDefault};

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum CoreBlockPyExprWithAwaitAndYield {
    Literal(LiteralValue),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    Call(Call<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    MakeCell(MakeCell<Self>),
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
    Await(Await<Self>),
    Yield(Yield<Self>),
    YieldFrom(YieldFrom<Self>),
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum CoreBlockPyExprWithYield {
    Literal(LiteralValue),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    Call(Call<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    MakeCell(MakeCell<Self>),
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
    Yield(Yield<Self>),
    YieldFrom(YieldFrom<Self>),
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum CoreBlockPyExpr<N: BlockPyNameLike = UnresolvedName> {
    Literal(LiteralValue),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    Call(Call<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    MakeCell(MakeCell<Self>),
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
}

pub type LocatedCoreBlockPyExpr = CoreBlockPyExpr<LocatedName>;

#[derive(Debug, Clone)]
pub struct CoreBlockPyPassWithAwaitAndYield;

impl BlockPyPass for CoreBlockPyPassWithAwaitAndYield {
    type Expr = CoreBlockPyExprWithAwaitAndYield;
}

#[derive(Debug, Clone)]
pub struct CoreBlockPyPassWithYield;

impl BlockPyPass for CoreBlockPyPassWithYield {
    type Expr = CoreBlockPyExprWithYield;
}

#[derive(Debug, Clone)]
pub struct CoreBlockPyPass;

impl BlockPyPass for CoreBlockPyPass {
    type Expr = CoreBlockPyExpr<UnresolvedName>;
}

#[derive(Debug, Clone)]
pub struct ResolvedStorageBlockPyPass;

impl BlockPyPass for ResolvedStorageBlockPyPass {
    type Expr = CoreBlockPyExpr<LocatedName>;
}

#[derive(Debug, Clone)]
pub struct CodegenBlockPyPass;

impl BlockPyPass for CodegenBlockPyPass {
    type Expr = CodegenBlockPyExpr;
}

pub(crate) use blockpy_generators::lower_yield_in_lowered_core_blockpy_module_bundle;
pub use blockpy_to_bb::{lower_try_jump_exception_flow, normalize_bb_module_strings};
pub use instr_id::{assign_function_instr_ids, assign_module_instr_ids};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
pub use trace::{
    instrument_bb_module_with_call_target_counters,
    instrument_bb_module_with_block_entry_counters, instrument_bb_module_with_global_load_counters,
    instrument_bb_module_with_refcount_counters,
};

pub(crate) use name_binding::lower_name_binding_in_core_blockpy_module;
pub(crate) use trace::{
    call_target_counter_instrumentation_enabled, global_load_counter_instrumentation_enabled,
    instrument_bb_module_for_trace, parse_trace_env,
};

pub fn relabel_dense_bb_module(module: &mut BlockPyModule<CodegenBlockPyPass>) {
    for callable in &mut module.callable_defs {
        relabel_blockpy_blocks_dense(&mut callable.blocks);
    }
}

#[cfg(test)]
mod test;
