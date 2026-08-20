use crate::block_py::{BuildCollection, CallArgumentOp, IteratorStep, PreparedCall};
pub(crate) mod ast_symbol_analysis;
pub(crate) mod ast_to_ast;
pub(crate) mod ast_to_instr;
pub(crate) mod block_parameter_roles;
pub(crate) mod blockpy_expr_simplify;
pub(crate) mod blockpy_generators;
pub(crate) mod blockpy_to_bb;
pub(crate) mod core_await_lower;
pub(crate) mod global_index;
pub(crate) mod name_binding;
pub(crate) mod ruff_to_blockpy;
pub(crate) mod strict_construction;

use crate::block_py::{
    runtime_name_load, ApplyClassDecorator, ApplyFunctionDescriptor, Await, BinOp, Call,
    CallArgKeyword, CallArgPositional, CellRef, CellRefForName, CheckAnnotationFormat,
    ChildVisitable, CompleteFunctionDefinition, ComprehensionInsert, ConstructTypeParameterScope,
    CreateTypeAlias, CreateTypeParameter, Del, DelItem, DiscardClassConstructionCaptures,
    DiscardClassDecorator, ExprAttribute, ExprBoolOp, ExprBooleanLiteral, ExprBytesLiteral,
    ExprCompare, ExprDict, ExprDictComp, ExprDictItem, ExprEllipsisLiteral, ExprFString,
    ExprGenerator, ExprIf, ExprIpyEscapeCommand, ExprLambda, ExprList, ExprListComp, ExprName,
    ExprNamed, ExprNoneLiteral, ExprNumberLiteral, ExprSet, ExprSetComp, ExprSlice, ExprStarred,
    ExprStringLiteral, ExprSubscript, ExprTString, ExprTuple, GetAttr, GetItem, HasMeta, Instr,
    InstrWithConstantNone, LiteralValue, Load, MakeCell, MakeFunction, MakeFunctionWithClosure,
    MapInstr, Mappable, Meta, ModuleShape, NameLike, NewAnnotationSet, PrepareClassDecorator,
    RecordAnnotation, ResolvedName, SetAttr, SetFunctionTypeParameters, SetItem,
    SetTypeParameterDefault, SetupAnnotations, StmtAnnAssign, StmtAssert, StmtAssign,
    StmtAugAssign, StmtBreak, StmtClassDef, StmtContinue, StmtDelete, StmtExpr, StmtFor,
    StmtFunctionDef, StmtGlobal, StmtIf, StmtImport, StmtImportFrom, StmtIpyEscapeCommand,
    StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn, StmtTry, StmtTypeAlias, StmtWhile,
    StmtWith, Store, SubscriptGeneric, TakeOperand, TryMapInstr, Tuple, UnaryOp, UnresolvedName,
    WithMeta, Yield, YieldFrom,
};
use ruff_python_ast::{self as ast};
use soac_macros::{enum_broadcast, DelegateMatchDefault};

/// BlockPy's Ruff-like phase, immediately after the AST-to-AST rewrite.
///
/// Compared with the previous phase, this is no longer a raw `ruff_python_ast`
/// tree: module and function bodies have been put into BlockPy containers, but
/// Python source constructs such as `while`, `try`, `with`, `await`, and
/// comprehensions are still represented directly. Names are still unresolved
/// source names. This is the bridge IR for syntax-directed lowering.
#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub(crate) enum InstrRuff {
    ExprBoolOp(ExprBoolOp<Self>),
    ExprNamed(ExprNamed<Self>),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    ExprLambda(ExprLambda<Self>),
    ExprIf(ExprIf<Self>),
    ExprDict(ExprDict<Self>),
    TakeOperand(TakeOperand<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    MakeCell(MakeCell<Self>),
    CellRefForName(CellRefForName),
    ComprehensionInsert(ComprehensionInsert<Self>),
    BuildCollection(BuildCollection<Self>),
    CallArgumentOp(CallArgumentOp<Self>),
    PreparedCall(PreparedCall<Self>),
    IteratorStep(IteratorStep<Self>),
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

impl Instr for InstrRuff {
    type Name = UnresolvedName;
    type Extra = ();
}

impl InstrWithConstantNone for InstrRuff {
    fn constant_none() -> Self {
        ExprNoneLiteral::new().into()
    }
}

/// Module shape for the Ruff-like BlockPy phase.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RuffModuleShape;

impl ModuleShape for RuffModuleShape {
    type Instr = InstrRuff;
    type ModuleConstant = InstrResolved;
    type BlockExtra = soac_core::block_py::BlockContext;
}

/// Core BlockPy after syntax-level control flow has been lowered to blocks.
///
/// Compared with `InstrRuff`, structured statements and expressions that affect
/// control flow have been converted into block edges and simpler operations.
/// `await`, `yield`, and `yield from` still remain because coroutine and
/// generator lowering happens in later phases. Names are still unresolved.
#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub(crate) enum InstrWithAwaitAndYield {
    Literal(LiteralValue),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    Tuple(Tuple<Self>),
    Call(Call<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    TakeOperand(TakeOperand<Self>),
    ComprehensionInsert(ComprehensionInsert<Self>),
    BuildCollection(BuildCollection<Self>),
    CallArgumentOp(CallArgumentOp<Self>),
    PreparedCall(PreparedCall<Self>),
    IteratorStep(IteratorStep<Self>),
    MakeCell(MakeCell<Self>),
    NewAnnotationSet(NewAnnotationSet<Self>),
    SetupAnnotations(SetupAnnotations<Self>),
    ConstructTypeParameterScope(ConstructTypeParameterScope<Self>),
    SubscriptGeneric(SubscriptGeneric<Self>),
    SetFunctionTypeParameters(SetFunctionTypeParameters<Self>),
    CreateTypeAlias(CreateTypeAlias<Self>),
    CreateTypeParameter(CreateTypeParameter<Self>),
    SetTypeParameterDefault(SetTypeParameterDefault<Self>),
    CheckAnnotationFormat(CheckAnnotationFormat<Self>),
    RecordAnnotation(RecordAnnotation<Self>),
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
    CompleteFunctionDefinition(CompleteFunctionDefinition<Self>),
    ApplyFunctionDescriptor(ApplyFunctionDescriptor<Self>),
    PrepareClassDecorator(PrepareClassDecorator<Self>),
    ApplyClassDecorator(ApplyClassDecorator<Self>),
    DiscardClassDecorator(DiscardClassDecorator<Self>),
    DiscardClassConstructionCaptures(DiscardClassConstructionCaptures<Self>),
    Await(Await<Self>),
    Yield(Yield<Self>),
    YieldFrom(YieldFrom<Self>),
}

impl soac_core::block_py::TakeOperandInstruction for InstrWithAwaitAndYield {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
        match self {
            Self::TakeOperand(op) => Some(op),
            _ => None,
        }
    }
}

impl Instr for InstrWithAwaitAndYield {
    type Name = UnresolvedName;
    type Extra = ();
}

impl InstrWithConstantNone for InstrWithAwaitAndYield {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

/// Module shape for core BlockPy before await/yield lowering.
#[derive(Debug, Clone)]
pub(crate) struct CoreModuleShapeWithAwaitAndYield;

impl ModuleShape for CoreModuleShapeWithAwaitAndYield {
    type Instr = InstrWithAwaitAndYield;
    type ModuleConstant = InstrResolved;
    type BlockExtra = soac_core::block_py::BlockContext;
}

/// Core BlockPy after `await` has been rewritten but generator yields remain.
///
/// Compared with `InstrWithAwaitAndYield`, `await expr` has been lowered into
/// the runtime await-iterator protocol, so only `yield` and `yield from` still
/// require generator/coroutine state-machine handling. Names are still
/// unresolved.
#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub(crate) enum InstrWithYield {
    Literal(LiteralValue),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    Tuple(Tuple<Self>),
    Call(Call<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    TakeOperand(TakeOperand<Self>),
    ComprehensionInsert(ComprehensionInsert<Self>),
    BuildCollection(BuildCollection<Self>),
    CallArgumentOp(CallArgumentOp<Self>),
    PreparedCall(PreparedCall<Self>),
    IteratorStep(IteratorStep<Self>),
    MakeCell(MakeCell<Self>),
    NewAnnotationSet(NewAnnotationSet<Self>),
    SetupAnnotations(SetupAnnotations<Self>),
    ConstructTypeParameterScope(ConstructTypeParameterScope<Self>),
    SubscriptGeneric(SubscriptGeneric<Self>),
    SetFunctionTypeParameters(SetFunctionTypeParameters<Self>),
    CreateTypeAlias(CreateTypeAlias<Self>),
    CreateTypeParameter(CreateTypeParameter<Self>),
    SetTypeParameterDefault(SetTypeParameterDefault<Self>),
    CheckAnnotationFormat(CheckAnnotationFormat<Self>),
    RecordAnnotation(RecordAnnotation<Self>),
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
    CompleteFunctionDefinition(CompleteFunctionDefinition<Self>),
    ApplyFunctionDescriptor(ApplyFunctionDescriptor<Self>),
    PrepareClassDecorator(PrepareClassDecorator<Self>),
    ApplyClassDecorator(ApplyClassDecorator<Self>),
    DiscardClassDecorator(DiscardClassDecorator<Self>),
    DiscardClassConstructionCaptures(DiscardClassConstructionCaptures<Self>),
    Yield(Yield<Self>),
    YieldFrom(YieldFrom<Self>),
}

impl soac_core::block_py::TakeOperandInstruction for InstrWithYield {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
        match self {
            Self::TakeOperand(op) => Some(op),
            _ => None,
        }
    }
}

impl Instr for InstrWithYield {
    type Name = UnresolvedName;
    type Extra = ();
}

impl InstrWithConstantNone for InstrWithYield {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

/// Module shape for core BlockPy after await lowering and before yield lowering.
#[derive(Debug, Clone)]
pub(crate) struct CoreModuleShapeWithYield;

impl ModuleShape for CoreModuleShapeWithYield {
    type Instr = InstrWithYield;
    type ModuleConstant = InstrResolved;
    type BlockExtra = soac_core::block_py::BlockContext;
}

/// Core BlockPy after generator/coroutine yield lowering.
///
/// Compared with `InstrWithYield`, yield points have been lowered into explicit
/// resume/state-machine structure, and functions now use only ordinary
/// expression, storage, call, and function-construction operations. Names are
/// still unresolved, so name binding has not yet chosen local/global/cell
/// storage.
#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub(crate) enum InstrLow<N: NameLike> {
    Literal(LiteralValue),
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    Tuple(Tuple<Self>),
    Call(Call<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    TakeOperand(TakeOperand<Self>),
    ComprehensionInsert(ComprehensionInsert<Self>),
    BuildCollection(BuildCollection<Self>),
    CallArgumentOp(CallArgumentOp<Self>),
    PreparedCall(PreparedCall<Self>),
    IteratorStep(IteratorStep<Self>),
    MakeCell(MakeCell<Self>),
    NewAnnotationSet(NewAnnotationSet<Self>),
    SetupAnnotations(SetupAnnotations<Self>),
    ConstructTypeParameterScope(ConstructTypeParameterScope<Self>),
    SubscriptGeneric(SubscriptGeneric<Self>),
    SetFunctionTypeParameters(SetFunctionTypeParameters<Self>),
    CreateTypeAlias(CreateTypeAlias<Self>),
    CreateTypeParameter(CreateTypeParameter<Self>),
    SetTypeParameterDefault(SetTypeParameterDefault<Self>),
    CheckAnnotationFormat(CheckAnnotationFormat<Self>),
    RecordAnnotation(RecordAnnotation<Self>),
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
    MakeFunctionWithClosure(MakeFunctionWithClosure<Self>),
    CompleteFunctionDefinition(CompleteFunctionDefinition<Self>),
    ApplyFunctionDescriptor(ApplyFunctionDescriptor<Self>),
    PrepareClassDecorator(PrepareClassDecorator<Self>),
    ApplyClassDecorator(ApplyClassDecorator<Self>),
    DiscardClassDecorator(DiscardClassDecorator<Self>),
    DiscardClassConstructionCaptures(DiscardClassConstructionCaptures<Self>),
}

impl<N: NameLike> soac_core::block_py::TakeOperandInstruction for InstrLow<N> {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
        match self {
            Self::TakeOperand(op) => Some(op),
            _ => None,
        }
    }
}

impl<N: NameLike> Instr for InstrLow<N> {
    type Name = N;
    type Extra = ();
}

impl<N: NameLike> InstrWithConstantNone for InstrLow<N> {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

pub(crate) type InstrUnresolved = InstrLow<UnresolvedName>;

/// Module shape for core BlockPy before name binding.
#[derive(Debug, Clone)]
pub(crate) struct CoreModuleShape;

impl ModuleShape for CoreModuleShape {
    type Instr = InstrUnresolved;
    type ModuleConstant = InstrResolved;
    type BlockExtra = soac_core::block_py::BlockContext;
}

/// BlockPy after name binding has resolved source names to storage locations.
///
/// Compared with `InstrUnresolved`, loads/stores/deletes no longer carry raw
/// source names: they refer to resolved local, global, constant, cell, closure,
/// or runtime locations. This shape is reused through global-indexing and
/// exception-edge CFG preparation until the final codegen-only operations are
/// introduced.
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
pub enum InstrResolved {
    Literal(LiteralValue),
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
    CellRef(CellRef),
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
    CompleteFunctionDefinition(#[rkyv(omit_bounds)] CompleteFunctionDefinition<Self>),
    ApplyFunctionDescriptor(#[rkyv(omit_bounds)] ApplyFunctionDescriptor<Self>),
    PrepareClassDecorator(#[rkyv(omit_bounds)] PrepareClassDecorator<Self>),
    ApplyClassDecorator(#[rkyv(omit_bounds)] ApplyClassDecorator<Self>),
    DiscardClassDecorator(#[rkyv(omit_bounds)] DiscardClassDecorator<Self>),
    DiscardClassConstructionCaptures(#[rkyv(omit_bounds)] DiscardClassConstructionCaptures<Self>),
}

impl soac_core::block_py::TakeOperandInstruction for InstrResolved {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
        match self {
            Self::TakeOperand(op) => Some(op),
            _ => None,
        }
    }
}

impl Instr for InstrResolved {
    type Name = ResolvedName;
    type Extra = ();
}

impl InstrWithConstantNone for InstrResolved {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

/// Module shape for resolved-storage BlockPy.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct ResolvedStorageModuleShape;

impl ModuleShape for ResolvedStorageModuleShape {
    type Instr = InstrResolved;
    type ModuleConstant = InstrResolved;
    type BlockExtra = soac_core::block_py::BlockContext;
}

#[cfg(test)]
mod test;
