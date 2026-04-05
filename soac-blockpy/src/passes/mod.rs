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
    CodegenBlockPyExpr, Del, DelItem, ExprAttribute, ExprBoolOp, ExprBooleanLiteral,
    ExprBytesLiteral, ExprCompare, ExprDict, ExprDictComp, ExprEllipsisLiteral, ExprFString,
    ExprGenerator, ExprIpyEscapeCommand, ExprIf, ExprLambda, ExprList, ExprListComp, ExprName,
    ExprNamed, ExprNoneLiteral, ExprNumberLiteral, ExprSet, ExprSetComp, ExprSlice, ExprStarred,
    ExprStringLiteral, ExprSubscript, ExprTString, ExprTuple, GetAttr, GetItem, HasMeta, Instr,
    Load, LiteralValue, MakeCell, MakeFunction, MapInstr, Mappable, Meta, ResolvedName, SetAttr,
    SetItem, StmtAnnAssign, StmtAssign, StmtAssert, StmtAugAssign, StmtBreak,
    StmtClassDef, StmtContinue, StmtDelete, StmtExpr, StmtFor, StmtFunctionDef, StmtGlobal,
    StmtIf, StmtImport, StmtImportFrom, StmtIpyEscapeCommand, StmtMatch, StmtNonlocal, StmtPass,
    StmtRaise, StmtReturn, StmtTry, StmtTypeAlias, StmtWhile, StmtWith, Store, TryMapInstr,
    UnaryOp, UnresolvedName, WithMeta, Yield, YieldFrom,
};
use ruff_python_ast::{self as ast};
use soac_macros::{enum_broadcast, DelegateMatchDefault};

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum InstrRuff {
    ExprBoolOp(ExprBoolOp<Self>),
    ExprNamed(ExprNamed<Self>),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    ExprLambda(ExprLambda<Self>),
    ExprIf(ExprIf<Self>),
    ExprDict(ExprDict),
    ExprSet(ExprSet<Self>),
    ExprListComp(ExprListComp<Self>),
    ExprSetComp(ExprSetComp<Self>),
    ExprDictComp(ExprDictComp<Self>),
    ExprGenerator(ExprGenerator<Self>),
    Await(Await<Self>),
    Yield(Yield<Self>),
    YieldFrom(YieldFrom<Self>),
    ExprCompare(ExprCompare<Self>),
    Call(Call<Self>),
    ExprFString(ExprFString),
    ExprTString(ExprTString),
    ExprStringLiteral(ExprStringLiteral),
    ExprBytesLiteral(ExprBytesLiteral),
    ExprNumberLiteral(ExprNumberLiteral),
    ExprBooleanLiteral(ExprBooleanLiteral),
    ExprNoneLiteral(ExprNoneLiteral),
    ExprEllipsisLiteral(ExprEllipsisLiteral),
    ExprAttribute(ExprAttribute<Self>),
    ExprSubscript(ExprSubscript<Self>),
    ExprStarred(ExprStarred<Self>),
    ExprName(ExprName),
    ExprList(ExprList<Self>),
    ExprTuple(ExprTuple<Self>),
    ExprSlice(ExprSlice<Self>),
    ExprIpyEscapeCommand(ExprIpyEscapeCommand),
    StmtFunctionDef(StmtFunctionDef<Self>),
    StmtClassDef(StmtClassDef<Self>),
    StmtReturn(StmtReturn<Self>),
    StmtDelete(StmtDelete<Self>),
    StmtTypeAlias(StmtTypeAlias<Self>),
    StmtAssign(StmtAssign<Self>),
    StmtAugAssign(StmtAugAssign<Self>),
    StmtAnnAssign(StmtAnnAssign<Self>),
    StmtFor(StmtFor<Self>),
    StmtWhile(StmtWhile<Self>),
    StmtIf(StmtIf<Self>),
    StmtWith(StmtWith<Self>),
    StmtMatch(StmtMatch<Self>),
    StmtRaise(StmtRaise<Self>),
    StmtTry(StmtTry<Self>),
    StmtAssert(StmtAssert<Self>),
    StmtImport(StmtImport),
    StmtImportFrom(StmtImportFrom),
    StmtGlobal(StmtGlobal),
    StmtNonlocal(StmtNonlocal),
    StmtExpr(StmtExpr<Self>),
    StmtPass(StmtPass),
    StmtBreak(StmtBreak),
    StmtContinue(StmtContinue),
    StmtIpyEscapeCommand(StmtIpyEscapeCommand),
}

#[derive(Debug, Clone)]
pub struct RuffBlockPyPass;

impl BlockPyPass for RuffBlockPyPass {
    type Expr = InstrRuff;
}

impl Instr for InstrRuff {
    type Name = ast::ExprName;
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum InstrWithAwaitAndYield {
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
pub enum InstrWithYield {
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
pub enum InstrLow<N: BlockPyNameLike> {
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

pub type InstrUnresolved = InstrLow<UnresolvedName>;
pub type InstrResolved = InstrLow<ResolvedName>;

#[derive(Debug, Clone)]
pub struct CoreBlockPyPassWithAwaitAndYield;

impl BlockPyPass for CoreBlockPyPassWithAwaitAndYield {
    type Expr = InstrWithAwaitAndYield;
}

#[derive(Debug, Clone)]
pub struct CoreBlockPyPassWithYield;

impl BlockPyPass for CoreBlockPyPassWithYield {
    type Expr = InstrWithYield;
}

#[derive(Debug, Clone)]
pub struct CoreBlockPyPass;

impl BlockPyPass for CoreBlockPyPass {
    type Expr = InstrLow<UnresolvedName>;
}

#[derive(Debug, Clone)]
pub struct ResolvedStorageBlockPyPass;

impl BlockPyPass for ResolvedStorageBlockPyPass {
    type Expr = InstrLow<ResolvedName>;
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
