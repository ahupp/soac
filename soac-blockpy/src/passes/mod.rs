pub(crate) mod ast_symbol_analysis;
pub(crate) mod ast_to_ast;
pub(crate) mod ast_to_instr;
pub(crate) mod blockpy_expr_simplify;
mod blockpy_generators;
pub mod blockpy_to_bb;
pub(crate) mod core_await_lower;
mod global_index;
mod instr_id;
mod instrument;
mod name_binding;
mod ownership_effects;
pub mod ruff_to_blockpy;
mod trace;
mod value_facts;

use crate::block_py::{
    cfg::relabel_blockpy_blocks_dense, runtime_name_load, Await, BinOp, BlockPyModule, Call,
    CallArgKeyword, CallArgPositional, CallDirect, CalleeFunctionId, CellRef, CellRefForName,
    ChildVisitable, Del, DelItem, ExprAttribute, ExprBoolOp, ExprBooleanLiteral, ExprBytesLiteral,
    ExprCompare, ExprDict, ExprDictComp, ExprEllipsisLiteral, ExprFString, ExprGenerator, ExprIf,
    ExprIpyEscapeCommand, ExprLambda, ExprList, ExprListComp, ExprName, ExprNamed, ExprNoneLiteral,
    ExprNumberLiteral, ExprSet, ExprSetComp, ExprSlice, ExprStarred, ExprStringLiteral,
    ExprSubscript, ExprTString, ExprTuple, GetAttr, GetItem, HasMeta, IdentifiedInstr,
    IncrementCounter, Instr, InstrWithConstantNone, LiteralValue, Load, MakeCell, MakeFunction,
    MapInstr, Mappable, Meta, ModuleShape, NameLike, ResolvedName, SetAttr, SetItem, StmtAnnAssign,
    StmtAssert, StmtAssign, StmtAugAssign, StmtBreak, StmtClassDef, StmtContinue, StmtDelete,
    StmtExpr, StmtFor, StmtFunctionDef, StmtGlobal, StmtIf, StmtImport, StmtImportFrom,
    StmtIpyEscapeCommand, StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn, StmtTry,
    StmtTypeAlias, StmtWhile, StmtWith, Store, TryMapInstr, UnaryOp, UnresolvedName, WithMeta,
    Yield, YieldFrom,
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
pub struct RuffModuleShape;

impl ModuleShape for RuffModuleShape {
    type Instr = InstrRuff;
}

impl Instr for InstrRuff {
    type Name = UnresolvedName;
}

impl InstrWithConstantNone for InstrRuff {
    fn constant_none() -> Self {
        ExprNoneLiteral::new().into()
    }
}

#[derive(Clone, derive_more::From, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(serialize_bounds(
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(bounds(
    __C: rkyv::validation::ArchiveContext,
)))]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum InstrCodegenOp {
    BinOp(#[rkyv(omit_bounds)] BinOp<Self>),
    UnaryOp(#[rkyv(omit_bounds)] UnaryOp<Self>),
    CalleeFunctionId(#[rkyv(omit_bounds)] CalleeFunctionId<Self>),
    Call(#[rkyv(omit_bounds)] Call<Self>),
    CallDirect(#[rkyv(omit_bounds)] CallDirect<Self>),
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
    MakeFunction(#[rkyv(omit_bounds)] MakeFunction<Self>),
}

pub type InstrCodegen = InstrCodegenOp;

impl Instr for InstrCodegenOp {
    type Name = ResolvedName;
}

impl<I> Instr for IdentifiedInstr<I>
where
    I: Instr,
{
    type Name = I::Name;
}

impl InstrWithConstantNone for InstrCodegenOp {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
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

impl Instr for InstrWithAwaitAndYield {
    type Name = UnresolvedName;
}

impl InstrWithConstantNone for InstrWithAwaitAndYield {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
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

impl Instr for InstrWithYield {
    type Name = UnresolvedName;
}

impl InstrWithConstantNone for InstrWithYield {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

#[derive(
    Clone,
    derive_more::From,
    DelegateMatchDefault,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(archive_bounds(N: rkyv::Archive))]
#[rkyv(serialize_bounds(
    N: rkyv::Serialize<__S>,
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(
    rkyv::Archived<N>: rkyv::Deserialize<N, __D>,
    __D::Error: rkyv::rancor::Source,
))]
#[rkyv(bytecheck(bounds(
    __C: rkyv::validation::ArchiveContext,
    rkyv::Archived<N>: rkyv::bytecheck::CheckBytes<__C>,
)))]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum InstrLow<N: NameLike> {
    Literal(LiteralValue),
    BinOp(#[rkyv(omit_bounds)] BinOp<Self>),
    UnaryOp(#[rkyv(omit_bounds)] UnaryOp<Self>),
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
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(#[rkyv(omit_bounds)] MakeFunction<Self>),
}

impl<N: NameLike> Instr for InstrLow<N> {
    type Name = N;
}

impl<N: NameLike> InstrWithConstantNone for InstrLow<N> {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

pub type InstrUnresolved = InstrLow<UnresolvedName>;

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
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum InstrResolved {
    Literal(LiteralValue),
    BinOp(#[rkyv(omit_bounds)] BinOp<Self>),
    UnaryOp(#[rkyv(omit_bounds)] UnaryOp<Self>),
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
    CellRef(CellRef),
    MakeFunction(#[rkyv(omit_bounds)] MakeFunction<Self>),
}

impl Instr for InstrResolved {
    type Name = ResolvedName;
}

impl InstrWithConstantNone for InstrResolved {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

#[derive(Debug, Clone)]
pub struct CoreModuleShapeWithAwaitAndYield;

impl ModuleShape for CoreModuleShapeWithAwaitAndYield {
    type Instr = InstrWithAwaitAndYield;
}

#[derive(Debug, Clone)]
pub struct CoreModuleShapeWithYield;

impl ModuleShape for CoreModuleShapeWithYield {
    type Instr = InstrWithYield;
}

#[derive(Debug, Clone)]
pub struct CoreModuleShape;

impl ModuleShape for CoreModuleShape {
    type Instr = InstrLow<UnresolvedName>;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ResolvedStorageModuleShape;

impl ModuleShape for ResolvedStorageModuleShape {
    type Instr = InstrResolved;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenModuleShape;

impl ModuleShape for CodegenModuleShape {
    type Instr = InstrCodegen;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenUnidentifiedModuleShape;

impl ModuleShape for CodegenUnidentifiedModuleShape {
    type Instr = InstrCodegen;
}

pub(crate) use blockpy_generators::lower_yield_in_lowered_core_blockpy_module_bundle;
pub use blockpy_to_bb::{lower_try_jump_exception_flow, normalize_bb_module_strings};
pub use instr_id::{
    assign_function_instr_ids, assign_module_instr_ids, validate_codegen_instr_ids,
};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
pub use ownership_effects::{
    plan_ownership_effects, validate_ownership_effects, BlockRefcountPlan, FunctionRefcountPlan,
    LocalRefState, RefcountAction, RefcountActionKind, RefcountLocal, RefcountPlan,
    RefcountReleaseReason, RefcountSite,
};
pub use trace::{
    instrument_bb_module_with_block_entry_counters, instrument_bb_module_with_call_target_counters,
    instrument_bb_module_with_global_load_counters, instrument_bb_module_with_locality_counters,
    instrument_bb_module_with_refcount_counters, specialization_runtime_logging_enabled,
};
pub use value_facts::{
    infer_module_value_facts, BoolFacts, BoolSingletonFact, CallableFact, EnvFacts, FactStore,
    I32Facts, I64Facts, NoneFact, ProvenanceFact, PyExactType, PyObjFacts, RefcountFact,
    RuntimeHelperId, RuntimeHelperSignature, RuntimeSingleton, ThrowSpec, TruthinessFact, TypeFact,
    ValueFacts,
};

pub(crate) use global_index::lower_global_index_in_resolved_module_default;
pub(crate) use name_binding::{
    lower_name_binding_in_core_blockpy_module,
    lower_name_binding_in_core_blockpy_module_with_options,
};
pub(crate) use trace::{
    call_target_counter_instrumentation_enabled, instrument_bb_module_for_trace,
    locality_counter_instrumentation_enabled, parse_trace_env,
};

pub fn relabel_dense_bb_module<P: ModuleShape>(module: &mut BlockPyModule<P>) {
    for callable in &mut module.callable_defs {
        relabel_blockpy_blocks_dense(&mut callable.blocks);
    }
}

#[cfg(test)]
mod test;
