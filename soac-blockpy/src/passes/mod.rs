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

use crate::block_py::{cfg::relabel_blockpy_blocks_dense, BlockPyModule, ImplicitNoneExpr};
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
    type Name = UnresolvedName;
}

impl ImplicitNoneExpr for InstrRuff {
    fn implicit_none_expr() -> Self {
        ExprNoneLiteral::new().into()
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(expr, Self::ExprNoneLiteral(_))
    }
}

impl InstrRuff {
    fn wrap_ast_expr<O>(meta: Meta, op: O) -> Self
    where
        O: WithMeta + Into<Self>,
    {
        op.with_meta(meta).into()
    }

    fn wrap_ast_stmt<O>(meta: Meta, op: O) -> Self
    where
        O: WithMeta + Into<Self>,
    {
        op.with_meta(meta).into()
    }

    fn none_expr_with_meta(meta: Meta) -> Self {
        ExprNoneLiteral::new().with_meta(meta).into()
    }

    fn from_ast_suite(body: Vec<ast::Stmt>) -> Vec<Self> {
        body.into_iter().map(Self::from_ast_stmt).collect()
    }

    fn into_ast_suite(body: Vec<Self>) -> Vec<ast::Stmt> {
        body.into_iter().map(Self::into_ast_stmt).collect()
    }

    fn normalize_if_orelse(elif_else_clauses: Vec<ast::ElifElseClause>) -> Vec<Self> {
        let mut clauses = elif_else_clauses.into_iter();
        let Some(first) = clauses.next() else {
            return Vec::new();
        };

        let ast::ElifElseClause {
            test,
            body,
            range,
            node_index,
        } = first;

        match test {
            Some(test) => {
                vec![Self::wrap_ast_stmt(
                    Meta::new(node_index, range),
                    StmtIf::new(
                        Self::from_ast_expr(test),
                        Self::from_ast_suite(body),
                        Self::normalize_if_orelse(clauses.collect()),
                    ),
                )]
            }
            None => Self::from_ast_suite(body),
        }
    }

    fn denormalize_if_orelse(orelse: Vec<Self>) -> Vec<ast::ElifElseClause> {
        if orelse.is_empty() {
            return Vec::new();
        }

        let mut iter = orelse.into_iter();
        match iter.next().expect("checked non-empty orelse") {
            Self::StmtIf(node) if iter.next().is_none() => {
                vec![ast::ElifElseClause {
                    range: node.meta().range,
                    node_index: node.meta().node_index,
                    test: Some(node.test.into_ast_expr()),
                    body: Self::into_ast_suite(node.body),
                }]
                .into_iter()
                .chain(Self::denormalize_if_orelse(node.orelse))
                .collect()
            }
            first => {
                let mut body = vec![first.into_ast_stmt()];
                body.extend(iter.map(Self::into_ast_stmt));
                vec![ast::ElifElseClause {
                    range: Default::default(),
                    node_index: ast::AtomicNodeIndex::default(),
                    test: None,
                    body,
                }]
            }
        }
    }

    pub fn into_ast_expr(self) -> ast::Expr {
        match self {
            Self::ExprBoolOp(node) => ast::Expr::BoolOp(ast::ExprBoolOp {
                range: node.meta().range,
                node_index: node.meta().node_index,
                op: node.op,
                values: node
                    .values
                    .into_iter()
                    .map(Self::into_ast_expr)
                    .collect::<Vec<_>>()
                    .into(),
            }),
            Self::ExprNamed(node) => ast::Expr::Named(ast::ExprNamed {
                range: node.meta().range,
                node_index: node.meta().node_index,
                target: Box::new(node.target.into_ast_expr()),
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::BinOp(node) => ast::Expr::BinOp(ast::ExprBinOp {
                range: node.meta().range,
                node_index: node.meta().node_index,
                left: Box::new(node.left.into_ast_expr()),
                op: node.kind.into_ast_operator(),
                right: Box::new(node.right.into_ast_expr()),
            }),
            Self::UnaryOp(node) => ast::Expr::UnaryOp(ast::ExprUnaryOp {
                range: node.meta().range,
                node_index: node.meta().node_index,
                op: node.kind.into_ast_unary_op(),
                operand: Box::new(node.operand.into_ast_expr()),
            }),
            Self::ExprLambda(node) => ast::Expr::Lambda(ast::ExprLambda {
                range: node.meta().range,
                node_index: node.meta().node_index,
                parameters: node.parameters,
                body: Box::new(node.body.into_ast_expr()),
            }),
            Self::ExprIf(node) => ast::Expr::If(ast::ExprIf {
                range: node.meta().range,
                node_index: node.meta().node_index,
                test: Box::new(node.test.into_ast_expr()),
                body: Box::new(node.body.into_ast_expr()),
                orelse: Box::new(node.orelse.into_ast_expr()),
            }),
            Self::ExprDict(node) => ast::Expr::Dict(ast::ExprDict {
                range: node.meta().range,
                node_index: node.meta().node_index,
                items: node.items,
            }),
            Self::ExprSet(node) => ast::Expr::Set(ast::ExprSet {
                range: node.meta().range,
                node_index: node.meta().node_index,
                elts: node
                    .elts
                    .into_iter()
                    .map(Self::into_ast_expr)
                    .collect::<Vec<_>>()
                    .into(),
            }),
            Self::ExprListComp(node) => ast::Expr::ListComp(ast::ExprListComp {
                range: node.meta().range,
                node_index: node.meta().node_index,
                elt: Box::new(node.elt.into_ast_expr()),
                generators: node.generators,
            }),
            Self::ExprSetComp(node) => ast::Expr::SetComp(ast::ExprSetComp {
                range: node.meta().range,
                node_index: node.meta().node_index,
                elt: Box::new(node.elt.into_ast_expr()),
                generators: node.generators,
            }),
            Self::ExprDictComp(node) => ast::Expr::DictComp(ast::ExprDictComp {
                range: node.meta().range,
                node_index: node.meta().node_index,
                key: Box::new(node.key.into_ast_expr()),
                value: Box::new(node.value.into_ast_expr()),
                generators: node.generators,
            }),
            Self::ExprGenerator(node) => ast::Expr::Generator(ast::ExprGenerator {
                range: node.meta().range,
                node_index: node.meta().node_index,
                elt: Box::new(node.elt.into_ast_expr()),
                generators: node.generators,
                parenthesized: node.parenthesized,
            }),
            Self::Await(node) => ast::Expr::Await(ast::ExprAwait {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::Yield(node) => ast::Expr::Yield(ast::ExprYield {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Some(Box::new(node.value.into_ast_expr())),
            }),
            Self::YieldFrom(node) => ast::Expr::YieldFrom(ast::ExprYieldFrom {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::ExprCompare(node) => ast::Expr::Compare(ast::ExprCompare {
                range: node.meta().range,
                node_index: node.meta().node_index,
                left: Box::new(node.left.into_ast_expr()),
                ops: node.ops.into(),
                comparators: node
                    .comparators
                    .into_iter()
                    .map(Self::into_ast_expr)
                    .collect::<Vec<_>>()
                    .into(),
            }),
            Self::Call(node) => ast::Expr::Call(ast::ExprCall {
                range: node.meta().range,
                node_index: node.meta().node_index,
                func: Box::new(node.func.into_ast_expr()),
                arguments: ast::Arguments {
                    range: Default::default(),
                    node_index: ast::AtomicNodeIndex::default(),
                    args: node
                        .args
                        .into_iter()
                        .map(|arg| match arg {
                            CallArgPositional::Positional(expr) => expr.into_ast_expr(),
                            CallArgPositional::Starred(expr) => ast::Expr::Starred(
                                ast::ExprStarred {
                                    range: expr.meta().range,
                                    node_index: expr.meta().node_index,
                                    value: Box::new(expr.into_ast_expr()),
                                    ctx: ast::ExprContext::Load,
                                },
                            ),
                        })
                        .collect::<Vec<_>>()
                        .into(),
                    keywords: node
                        .keywords
                        .into_iter()
                        .map(|keyword| match keyword {
                            CallArgKeyword::Named { arg, value } => ast::Keyword {
                                range: value.meta().range,
                                node_index: value.meta().node_index,
                                arg: Some(arg),
                                value: value.into_ast_expr(),
                            },
                            CallArgKeyword::Starred(value) => ast::Keyword {
                                range: value.meta().range,
                                node_index: value.meta().node_index,
                                arg: None,
                                value: value.into_ast_expr(),
                            },
                        })
                        .collect::<Vec<_>>()
                        .into(),
                },
            }),
            Self::ExprFString(node) => ast::Expr::FString(ast::ExprFString {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }),
            Self::ExprTString(node) => ast::Expr::TString(ast::ExprTString {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }),
            Self::ExprStringLiteral(node) => ast::Expr::StringLiteral(ast::ExprStringLiteral {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }),
            Self::ExprBytesLiteral(node) => ast::Expr::BytesLiteral(ast::ExprBytesLiteral {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }),
            Self::ExprNumberLiteral(node) => ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }),
            Self::ExprBooleanLiteral(node) => ast::Expr::BooleanLiteral(ast::ExprBooleanLiteral {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }),
            Self::ExprNoneLiteral(node) => ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
                range: node.meta().range,
                node_index: node.meta().node_index,
            }),
            Self::ExprEllipsisLiteral(node) => ast::Expr::EllipsisLiteral(ast::ExprEllipsisLiteral {
                range: node.meta().range,
                node_index: node.meta().node_index,
            }),
            Self::ExprAttribute(node) => ast::Expr::Attribute(ast::ExprAttribute {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Box::new(node.value.into_ast_expr()),
                attr: node.attr,
                ctx: node.ctx,
            }),
            Self::ExprSubscript(node) => ast::Expr::Subscript(ast::ExprSubscript {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Box::new(node.value.into_ast_expr()),
                slice: Box::new(node.slice.into_ast_expr()),
                ctx: node.ctx,
            }),
            Self::ExprStarred(node) => ast::Expr::Starred(ast::ExprStarred {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Box::new(node.value.into_ast_expr()),
                ctx: node.ctx,
            }),
            Self::ExprName(node) => ast::Expr::Name(ast::ExprName {
                range: node.meta().range,
                node_index: node.meta().node_index,
                id: node.id,
                ctx: node.ctx,
            }),
            Self::ExprList(node) => ast::Expr::List(ast::ExprList {
                range: node.meta().range,
                node_index: node.meta().node_index,
                elts: node
                    .elts
                    .into_iter()
                    .map(Self::into_ast_expr)
                    .collect::<Vec<_>>()
                    .into(),
                ctx: node.ctx,
            }),
            Self::ExprTuple(node) => ast::Expr::Tuple(ast::ExprTuple {
                range: node.meta().range,
                node_index: node.meta().node_index,
                elts: node
                    .elts
                    .into_iter()
                    .map(Self::into_ast_expr)
                    .collect::<Vec<_>>()
                    .into(),
                ctx: node.ctx,
                parenthesized: node.parenthesized,
            }),
            Self::ExprSlice(node) => ast::Expr::Slice(ast::ExprSlice {
                range: node.meta().range,
                node_index: node.meta().node_index,
                lower: node.lower.map(|expr| Box::new(expr.into_ast_expr())),
                upper: node.upper.map(|expr| Box::new(expr.into_ast_expr())),
                step: node.step.map(|expr| Box::new(expr.into_ast_expr())),
            }),
            Self::ExprIpyEscapeCommand(node) => {
                ast::Expr::IpyEscapeCommand(ast::ExprIpyEscapeCommand {
                    range: node.meta().range,
                    node_index: node.meta().node_index,
                    kind: node.kind,
                    value: node.value,
                })
            }
            other => panic!("expected expression-shaped InstrRuff, got {other:?}"),
        }
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
    pub fn from_ast_stmt(value: ast::Stmt) -> Self {
        match value {
            ast::Stmt::FunctionDef(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtFunctionDef::new(
                        node.is_async,
                        node.decorator_list,
                        node.name,
                        node.type_params,
                        node.parameters,
                        node.returns.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                        Self::from_ast_suite(node.body),
                    ),
                )
            }
            ast::Stmt::ClassDef(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtClassDef::new(
                        node.decorator_list,
                        node.name,
                        node.type_params,
                        node.arguments,
                        Self::from_ast_suite(node.body),
                    ),
                )
            }
            ast::Stmt::Return(node) => {
                let meta = node.meta();
                let implicit_none = Box::new(Self::none_expr_with_meta(meta.clone()));
                Self::wrap_ast_stmt(
                    meta,
                    StmtReturn::new(
                        node.value
                            .map(|expr| Box::new(Self::from_ast_expr(*expr)))
                            .unwrap_or(implicit_none),
                    ),
                )
            }
            ast::Stmt::Delete(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtDelete::new(node.targets.into_iter().map(Self::from_ast_expr).collect::<Vec<_>>()),
                )
            }
            ast::Stmt::TypeAlias(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtTypeAlias::new(
                        Self::from_ast_expr(*node.name),
                        node.type_params,
                        Self::from_ast_expr(*node.value),
                    ),
                )
            }
            ast::Stmt::Assign(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtAssign::new(
                        node.targets.into_iter().map(Self::from_ast_expr).collect::<Vec<_>>(),
                        Self::from_ast_expr(*node.value),
                    ),
                )
            }
            ast::Stmt::AugAssign(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtAugAssign::new(
                        Self::from_ast_expr(*node.target),
                        node.op,
                        Self::from_ast_expr(*node.value),
                    ),
                )
            }
            ast::Stmt::AnnAssign(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtAnnAssign::new(
                        Self::from_ast_expr(*node.target),
                        Self::from_ast_expr(*node.annotation),
                        node.value.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                        node.simple,
                    ),
                )
            }
            ast::Stmt::For(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtFor::new(
                        node.is_async,
                        Self::from_ast_expr(*node.target),
                        Self::from_ast_expr(*node.iter),
                        Self::from_ast_suite(node.body),
                        Self::from_ast_suite(node.orelse),
                    ),
                )
            }
            ast::Stmt::While(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtWhile::new(
                        Self::from_ast_expr(*node.test),
                        Self::from_ast_suite(node.body),
                        Self::from_ast_suite(node.orelse),
                    ),
                )
            }
            ast::Stmt::If(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtIf::new(
                        Self::from_ast_expr(*node.test),
                        Self::from_ast_suite(node.body),
                        Self::normalize_if_orelse(node.elif_else_clauses),
                    ),
                )
            }
            ast::Stmt::With(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtWith::new(
                        node.is_async,
                        node.items,
                        Self::from_ast_suite(node.body),
                    ),
                )
            }
            ast::Stmt::Match(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtMatch::new(Self::from_ast_expr(*node.subject), node.cases),
                )
            }
            ast::Stmt::Raise(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtRaise::new(
                        node.exc.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                        node.cause.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                    ),
                )
            }
            ast::Stmt::Try(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtTry::new(
                        Self::from_ast_suite(node.body),
                        node.handlers,
                        Self::from_ast_suite(node.orelse),
                        Self::from_ast_suite(node.finalbody),
                        node.is_star,
                    ),
                )
            }
            ast::Stmt::Assert(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(
                    meta,
                    StmtAssert::new(
                        Self::from_ast_expr(*node.test),
                        node.msg.map(|expr| Box::new(Self::from_ast_expr(*expr))),
                    ),
                )
            }
            ast::Stmt::Import(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtImport::new(node.names))
            }
            ast::Stmt::ImportFrom(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtImportFrom::new(node.module, node.names, node.level))
            }
            ast::Stmt::Global(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtGlobal::new(node.names))
            }
            ast::Stmt::Nonlocal(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtNonlocal::new(node.names))
            }
            ast::Stmt::Expr(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtExpr::new(Self::from_ast_expr(*node.value)))
            }
            ast::Stmt::Pass(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtPass::new())
            }
            ast::Stmt::Break(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtBreak::new())
            }
            ast::Stmt::Continue(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtContinue::new())
            }
            ast::Stmt::IpyEscapeCommand(node) => {
                let meta = node.meta();
                Self::wrap_ast_stmt(meta, StmtIpyEscapeCommand::new(node.kind, node.value))
            }
        }
    }

    pub fn into_ast_stmt(self) -> ast::Stmt {
        match self {
            Self::StmtFunctionDef(node) => ast::Stmt::FunctionDef(ast::StmtFunctionDef {
                range: node.meta().range,
                node_index: node.meta().node_index,
                is_async: node.is_async,
                decorator_list: node.decorator_list,
                name: node.name,
                type_params: node.type_params,
                parameters: node.parameters,
                returns: node.returns.map(|expr| Box::new(expr.into_ast_expr())),
                body: Self::into_ast_suite(node.body),
            }),
            Self::StmtClassDef(node) => ast::Stmt::ClassDef(ast::StmtClassDef {
                range: node.meta().range,
                node_index: node.meta().node_index,
                decorator_list: node.decorator_list,
                name: node.name,
                type_params: node.type_params,
                arguments: node.arguments,
                body: Self::into_ast_suite(node.body),
            }),
            Self::StmtReturn(node) => ast::Stmt::Return(ast::StmtReturn {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Some(Box::new(node.value.into_ast_expr())),
            }),
            Self::StmtDelete(node) => ast::Stmt::Delete(ast::StmtDelete {
                range: node.meta().range,
                node_index: node.meta().node_index,
                targets: node.targets.into_iter().map(Self::into_ast_expr).collect(),
            }),
            Self::StmtTypeAlias(node) => ast::Stmt::TypeAlias(ast::StmtTypeAlias {
                range: node.meta().range,
                node_index: node.meta().node_index,
                name: Box::new(node.name.into_ast_expr()),
                type_params: node.type_params,
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::StmtAssign(node) => ast::Stmt::Assign(ast::StmtAssign {
                range: node.meta().range,
                node_index: node.meta().node_index,
                targets: node.targets.into_iter().map(Self::into_ast_expr).collect(),
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::StmtAugAssign(node) => ast::Stmt::AugAssign(ast::StmtAugAssign {
                range: node.meta().range,
                node_index: node.meta().node_index,
                target: Box::new(node.target.into_ast_expr()),
                op: node.op,
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::StmtAnnAssign(node) => ast::Stmt::AnnAssign(ast::StmtAnnAssign {
                range: node.meta().range,
                node_index: node.meta().node_index,
                target: Box::new(node.target.into_ast_expr()),
                annotation: Box::new(node.annotation.into_ast_expr()),
                value: node.value.map(|expr| Box::new(expr.into_ast_expr())),
                simple: node.simple,
            }),
            Self::StmtFor(node) => ast::Stmt::For(ast::StmtFor {
                range: node.meta().range,
                node_index: node.meta().node_index,
                is_async: node.is_async,
                target: Box::new(node.target.into_ast_expr()),
                iter: Box::new(node.iter.into_ast_expr()),
                body: Self::into_ast_suite(node.body),
                orelse: Self::into_ast_suite(node.orelse),
            }),
            Self::StmtWhile(node) => ast::Stmt::While(ast::StmtWhile {
                range: node.meta().range,
                node_index: node.meta().node_index,
                test: Box::new(node.test.into_ast_expr()),
                body: Self::into_ast_suite(node.body),
                orelse: Self::into_ast_suite(node.orelse),
            }),
            Self::StmtIf(node) => ast::Stmt::If(ast::StmtIf {
                range: node.meta().range,
                node_index: node.meta().node_index,
                test: Box::new(node.test.into_ast_expr()),
                body: Self::into_ast_suite(node.body),
                elif_else_clauses: Self::denormalize_if_orelse(node.orelse),
            }),
            Self::StmtWith(node) => ast::Stmt::With(ast::StmtWith {
                range: node.meta().range,
                node_index: node.meta().node_index,
                is_async: node.is_async,
                items: node.items,
                body: Self::into_ast_suite(node.body),
            }),
            Self::StmtMatch(node) => ast::Stmt::Match(ast::StmtMatch {
                range: node.meta().range,
                node_index: node.meta().node_index,
                subject: Box::new(node.subject.into_ast_expr()),
                cases: node.cases,
            }),
            Self::StmtRaise(node) => ast::Stmt::Raise(ast::StmtRaise {
                range: node.meta().range,
                node_index: node.meta().node_index,
                exc: node.exc.map(|expr| Box::new(expr.into_ast_expr())),
                cause: node.cause.map(|expr| Box::new(expr.into_ast_expr())),
            }),
            Self::StmtTry(node) => ast::Stmt::Try(ast::StmtTry {
                range: node.meta().range,
                node_index: node.meta().node_index,
                body: Self::into_ast_suite(node.body),
                handlers: node.handlers,
                orelse: Self::into_ast_suite(node.orelse),
                finalbody: Self::into_ast_suite(node.finalbody),
                is_star: node.is_star,
            }),
            Self::StmtAssert(node) => ast::Stmt::Assert(ast::StmtAssert {
                range: node.meta().range,
                node_index: node.meta().node_index,
                test: Box::new(node.test.into_ast_expr()),
                msg: node.msg.map(|expr| Box::new(expr.into_ast_expr())),
            }),
            Self::StmtImport(node) => ast::Stmt::Import(ast::StmtImport {
                range: node.meta().range,
                node_index: node.meta().node_index,
                names: node.names,
            }),
            Self::StmtImportFrom(node) => ast::Stmt::ImportFrom(ast::StmtImportFrom {
                range: node.meta().range,
                node_index: node.meta().node_index,
                module: node.module,
                names: node.names,
                level: node.level,
            }),
            Self::StmtGlobal(node) => ast::Stmt::Global(ast::StmtGlobal {
                range: node.meta().range,
                node_index: node.meta().node_index,
                names: node.names,
            }),
            Self::StmtNonlocal(node) => ast::Stmt::Nonlocal(ast::StmtNonlocal {
                range: node.meta().range,
                node_index: node.meta().node_index,
                names: node.names,
            }),
            Self::StmtExpr(node) => ast::Stmt::Expr(ast::StmtExpr {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: Box::new(node.value.into_ast_expr()),
            }),
            Self::StmtPass(node) => ast::Stmt::Pass(ast::StmtPass {
                range: node.meta().range,
                node_index: node.meta().node_index,
            }),
            Self::StmtBreak(node) => ast::Stmt::Break(ast::StmtBreak {
                range: node.meta().range,
                node_index: node.meta().node_index,
            }),
            Self::StmtContinue(node) => ast::Stmt::Continue(ast::StmtContinue {
                range: node.meta().range,
                node_index: node.meta().node_index,
            }),
            Self::StmtIpyEscapeCommand(node) => ast::Stmt::IpyEscapeCommand(
                ast::StmtIpyEscapeCommand {
                    range: node.meta().range,
                    node_index: node.meta().node_index,
                    kind: node.kind,
                    value: node.value,
                },
            ),
            other => panic!("expected statement-shaped InstrRuff, got {other:?}"),
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
