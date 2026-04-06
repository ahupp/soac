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
    Await, BinOp, BlockPyNameLike, BlockPyPass, Call, CallArgKeyword, CallArgPositional, CellRef,
    CellRefForName, ChildVisitable, CodegenBlockPyExpr, Del, DelItem, ExprAttribute, ExprBoolOp,
    ExprBooleanLiteral, ExprBytesLiteral, ExprCompare, ExprDict, ExprDictComp,
    ExprEllipsisLiteral, ExprFString, ExprGenerator, ExprIpyEscapeCommand, ExprIf, ExprLambda,
    ExprList, ExprListComp, ExprName, ExprNamed, ExprNoneLiteral, ExprNumberLiteral, ExprSet,
    ExprSetComp, ExprSlice, ExprStarred, ExprStringLiteral, ExprSubscript, ExprTString,
    ExprTuple, GetAttr, GetItem, HasMeta, Instr, Load, LiteralValue, MakeCell, MakeFunction,
    MapInstr, Mappable, Meta, ResolvedName, SetAttr, SetItem, StmtAnnAssign, StmtAssign,
    StmtAssert, StmtAugAssign, StmtBreak,
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

impl InstrRuff {
    fn wrap_ast_expr<O>(meta: Meta, op: O) -> Self
    where
        O: WithMeta + Into<Self>,
    {
        op.with_meta(meta).into()
    }

    fn none_expr_with_meta(meta: Meta) -> Self {
        ExprNoneLiteral::new().with_meta(meta).into()
    }

    pub fn from_ast_expr(value: ast::Expr) -> Self {
        match value {
            ast::Expr::BoolOp(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprBoolOp::new(
                        node.op,
                        node.values
                            .into_iter()
                            .map(Self::from_ast_expr)
                            .collect::<Vec<_>>(),
                    ),
                )
            }
            ast::Expr::Named(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprNamed::new(Self::from_ast_expr(*node.target), Self::from_ast_expr(*node.value)),
                )
            }
            ast::Expr::BinOp(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    BinOp::new(
                        crate::block_py::operation::BinOpKind::from_ast_operator(node.op),
                        Self::from_ast_expr(*node.left),
                        Self::from_ast_expr(*node.right),
                    ),
                )
            }
            ast::Expr::UnaryOp(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    UnaryOp::new(
                        crate::block_py::operation::UnaryOpKind::from_ast_unary_op(node.op),
                        Self::from_ast_expr(*node.operand),
                    ),
                )
            }
            ast::Expr::Lambda(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprLambda::new(node.parameters, Self::from_ast_expr(*node.body)),
                )
            }
            ast::Expr::If(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprIf::new(
                        Self::from_ast_expr(*node.test),
                        Self::from_ast_expr(*node.body),
                        Self::from_ast_expr(*node.orelse),
                    ),
                )
            }
            ast::Expr::Dict(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprDict::new(node.items))
            }
            ast::Expr::Set(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprSet::new(
                        node.elts
                            .into_iter()
                            .map(Self::from_ast_expr)
                            .collect::<Vec<_>>(),
                    ),
                )
            }
            ast::Expr::ListComp(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprListComp::new(Self::from_ast_expr(*node.elt), node.generators),
                )
            }
            ast::Expr::SetComp(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprSetComp::new(Self::from_ast_expr(*node.elt), node.generators),
                )
            }
            ast::Expr::DictComp(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprDictComp::new(
                        Self::from_ast_expr(*node.key),
                        Self::from_ast_expr(*node.value),
                        node.generators,
                    ),
                )
            }
            ast::Expr::Generator(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprGenerator::new(
                        Self::from_ast_expr(*node.elt),
                        node.generators,
                        node.parenthesized,
                    ),
                )
            }
            ast::Expr::Await(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, Await::new(Self::from_ast_expr(*node.value)))
            }
            ast::Expr::Yield(node) => {
                let meta = node.meta();
                let fallback = Self::none_expr_with_meta(meta.clone());
                let value = node
                    .value
                    .map(|value| Self::from_ast_expr(*value))
                    .unwrap_or(fallback);
                Self::wrap_ast_expr(meta, Yield::new(value))
            }
            ast::Expr::YieldFrom(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, YieldFrom::new(Self::from_ast_expr(*node.value)))
            }
            ast::Expr::Compare(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprCompare::new(
                        Self::from_ast_expr(*node.left),
                        node.ops.into_vec(),
                        node.comparators
                            .into_vec()
                            .into_iter()
                            .map(Self::from_ast_expr)
                            .collect::<Vec<_>>(),
                    ),
                )
            }
            ast::Expr::Call(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    Call::new(
                        Self::from_ast_expr(*node.func),
                        node.arguments
                            .args
                            .into_vec()
                            .into_iter()
                            .map(|arg| {
                                CallArgPositional::from_ast_expr_with(arg, Self::from_ast_expr)
                            })
                            .collect::<Vec<_>>(),
                        node.arguments
                            .keywords
                            .into_vec()
                            .into_iter()
                            .map(|keyword| {
                                CallArgKeyword::from_ast_keyword_with(keyword, Self::from_ast_expr)
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
            }
            ast::Expr::FString(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprFString::new(node.value))
            }
            ast::Expr::TString(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprTString::new(node.value))
            }
            ast::Expr::StringLiteral(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprStringLiteral::new(node.value))
            }
            ast::Expr::BytesLiteral(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprBytesLiteral::new(node.value))
            }
            ast::Expr::NumberLiteral(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprNumberLiteral::new(node.value))
            }
            ast::Expr::BooleanLiteral(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprBooleanLiteral::new(node.value))
            }
            ast::Expr::NoneLiteral(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprNoneLiteral::new())
            }
            ast::Expr::EllipsisLiteral(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprEllipsisLiteral::new())
            }
            ast::Expr::Attribute(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprAttribute::new(Self::from_ast_expr(*node.value), node.attr, node.ctx),
                )
            }
            ast::Expr::Subscript(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprSubscript::new(
                        Self::from_ast_expr(*node.value),
                        Self::from_ast_expr(*node.slice),
                        node.ctx,
                    ),
                )
            }
            ast::Expr::Starred(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprStarred::new(Self::from_ast_expr(*node.value), node.ctx),
                )
            }
            ast::Expr::Name(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprName::new(node.id, node.ctx))
            }
            ast::Expr::List(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprList::new(
                        node.elts
                            .into_iter()
                            .map(Self::from_ast_expr)
                            .collect::<Vec<_>>(),
                        node.ctx,
                    ),
                )
            }
            ast::Expr::Tuple(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprTuple::new(
                        node.elts
                            .into_iter()
                            .map(Self::from_ast_expr)
                            .collect::<Vec<_>>(),
                        node.ctx,
                        node.parenthesized,
                    ),
                )
            }
            ast::Expr::Slice(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(
                    meta,
                    ExprSlice::new(
                        node.lower.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                        node.upper.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                        node.step.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                    ),
                )
            }
            ast::Expr::IpyEscapeCommand(node) => {
                let meta = node.meta();
                Self::wrap_ast_expr(meta, ExprIpyEscapeCommand::new(node.kind, node.value))
            }
        }
    }
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
