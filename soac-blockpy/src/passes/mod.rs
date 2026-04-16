pub(crate) mod ast_symbol_analysis;
pub(crate) mod ast_to_ast;
pub(crate) mod ast_to_instr;
pub(crate) mod blockpy_expr_simplify;
mod blockpy_generators;
pub mod blockpy_to_bb;
pub(crate) mod core_await_lower;
mod escape_analysis;
mod global_index;
mod inline_plan;
mod inline_sites;
mod inline_transform;
mod instr_id;
mod instrument;
mod local_env_plan;
mod name_binding;
mod ownership_effects;
pub mod ruff_to_blockpy;
mod trace;
mod value_facts;

use crate::block_py::{
    cfg::relabel_blockpy_blocks_dense, runtime_name_load, Await, BinOp, BlockPyFunction,
    BlockPyModule, BlockTerm, Call, CallArgKeyword, CallArgPositional, CallDirect,
    CalleeFunctionId, CellRef, CellRefForName, ChildVisitable, Del, DelItem, ExprAttribute,
    ExprBoolOp, ExprBooleanLiteral, ExprBytesLiteral, ExprCompare, ExprDict, ExprDictComp,
    ExprEllipsisLiteral, ExprFString, ExprGenerator, ExprIf, ExprIpyEscapeCommand, ExprLambda,
    ExprList, ExprListComp, ExprName, ExprNamed, ExprNoneLiteral, ExprNumberLiteral, ExprSet,
    ExprSetComp, ExprSlice, ExprStarred, ExprStringLiteral, ExprSubscript, ExprTString, ExprTuple,
    GetAttr, GetItem, HasMeta, IdentifiedInstr, IncrementCounter, Instr, InstrWithConstantNone,
    LiteralValue, Load, MakeCell, MakeFunction, MakeFunctionWithClosure, MapFunction, MapInstr,
    MapModule, Mappable, Meta, ModuleShape, NameLike, ResolvedName, SetAttr, SetItem,
    StmtAnnAssign, StmtAssert, StmtAssign, StmtAugAssign, StmtBreak, StmtClassDef, StmtContinue,
    StmtDelete, StmtExpr, StmtFor, StmtFunctionDef, StmtGlobal, StmtIf, StmtImport, StmtImportFrom,
    StmtIpyEscapeCommand, StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn, StmtTry,
    StmtTypeAlias, StmtWhile, StmtWith, Store, TryMapInstr, TryMapModule, TryMapTerm, Tuple,
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
    Tuple(#[rkyv(omit_bounds)] Tuple<Self>),
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
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
}

pub type InstrCodegen = InstrCodegenOp;

impl Instr for InstrCodegenOp {
    type Name = ResolvedName;
}

#[derive(Clone)]
pub struct TypedTruthy<E: Instr> {
    _meta: Meta,
    value: Box<E>,
}

impl<E: Instr> TypedTruthy<E> {
    pub fn new(value: impl Into<Box<E>>) -> Self {
        Self {
            _meta: Meta::default(),
            value: value.into(),
        }
    }

    pub fn value(&self) -> &E {
        &self.value
    }

    pub fn into_value(self) -> E {
        *self.value
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedTruthy<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TypedTruthy").field(&self.value).finish()
    }
}

impl<E: Instr> HasMeta for TypedTruthy<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for TypedTruthy<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for TypedTruthy<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.value);
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.value);
    }
}

impl<E: Instr> Mappable<E> for TypedTruthy<E> {
    type Mapped<T: Instr> = TypedTruthy<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        TypedTruthy {
            _meta: self._meta,
            value: Box::new(map.map_instr(*self.value)),
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(TypedTruthy {
            _meta: self._meta,
            value: Box::new(map.try_map_instr(*self.value)?),
        })
    }
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum InstrTyped {
    Truthy(TypedTruthy<Self>),
    Load(Load<Self>),
    BinOp(BinOp<Self>),
    LegacyTuple(Tuple<Self>),
    LegacyUnaryOp(UnaryOp<Self>),
    LegacyCalleeFunctionId(CalleeFunctionId<Self>),
    LegacyCall(Call<Self>),
    LegacyCallDirect(CallDirect<Self>),
    LegacyGetAttr(GetAttr<Self>),
    LegacySetAttr(SetAttr<Self>),
    LegacyGetItem(GetItem<Self>),
    LegacySetItem(SetItem<Self>),
    LegacyDelItem(DelItem<Self>),
    LegacyStore(Store<Self>),
    LegacyDel(Del<Self>),
    LegacyMakeCell(MakeCell<Self>),
    LegacyIncrementCounter(IncrementCounter),
    LegacyCellRef(CellRef),
    LegacyMakeFunctionWithClosure(MakeFunctionWithClosure<Self>),
}

pub type InstrTypedCodegen = InstrTyped;

impl InstrTyped {
    pub fn is_legacy(&self) -> bool {
        matches!(
            self,
            Self::LegacyUnaryOp(_)
                | Self::LegacyCalleeFunctionId(_)
                | Self::LegacyCall(_)
                | Self::LegacyTuple(_)
                | Self::LegacyCallDirect(_)
                | Self::LegacyGetAttr(_)
                | Self::LegacySetAttr(_)
                | Self::LegacyGetItem(_)
                | Self::LegacySetItem(_)
                | Self::LegacyDelItem(_)
                | Self::LegacyStore(_)
                | Self::LegacyDel(_)
                | Self::LegacyMakeCell(_)
                | Self::LegacyIncrementCounter(_)
                | Self::LegacyCellRef(_)
                | Self::LegacyMakeFunctionWithClosure(_)
        )
    }
}

impl Instr for InstrTyped {
    type Name = ResolvedName;
}

impl InstrWithConstantNone for InstrTyped {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

struct CodegenToTyped;

impl MapInstr<InstrCodegen, InstrTyped> for CodegenToTyped {
    fn map_instr(&mut self, instr: InstrCodegen) -> InstrTyped {
        match instr {
            InstrCodegenOp::BinOp(op) => InstrTyped::BinOp(op.map_children(self)),
            InstrCodegenOp::Tuple(op) => InstrTyped::LegacyTuple(op.map_children(self)),
            InstrCodegenOp::UnaryOp(op) => InstrTyped::LegacyUnaryOp(op.map_children(self)),
            InstrCodegenOp::CalleeFunctionId(op) => {
                InstrTyped::LegacyCalleeFunctionId(op.map_children(self))
            }
            InstrCodegenOp::Call(op) => InstrTyped::LegacyCall(op.map_children(self)),
            InstrCodegenOp::CallDirect(op) => InstrTyped::LegacyCallDirect(op.map_children(self)),
            InstrCodegenOp::GetAttr(op) => InstrTyped::LegacyGetAttr(op.map_children(self)),
            InstrCodegenOp::SetAttr(op) => InstrTyped::LegacySetAttr(op.map_children(self)),
            InstrCodegenOp::GetItem(op) => InstrTyped::LegacyGetItem(op.map_children(self)),
            InstrCodegenOp::SetItem(op) => InstrTyped::LegacySetItem(op.map_children(self)),
            InstrCodegenOp::DelItem(op) => InstrTyped::LegacyDelItem(op.map_children(self)),
            InstrCodegenOp::Load(op) => InstrTyped::Load(op.map_children(self)),
            InstrCodegenOp::Store(op) => InstrTyped::LegacyStore(op.map_children(self)),
            InstrCodegenOp::Del(op) => InstrTyped::LegacyDel(op.map_children(self)),
            InstrCodegenOp::MakeCell(op) => InstrTyped::LegacyMakeCell(op.map_children(self)),
            InstrCodegenOp::IncrementCounter(op) => InstrTyped::LegacyIncrementCounter(op),
            InstrCodegenOp::CellRef(op) => InstrTyped::LegacyCellRef(op),
            InstrCodegenOp::MakeFunctionWithClosure(op) => {
                InstrTyped::LegacyMakeFunctionWithClosure(op.map_children(self))
            }
        }
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}

pub fn lower_codegen_module_to_typed(
    module: BlockPyModule<CodegenModuleShape>,
) -> BlockPyModule<TypedCodegenModuleShape> {
    CodegenToTyped.map_module(module)
}

pub fn lower_codegen_function_to_typed(
    function: BlockPyFunction<CodegenModuleShape>,
) -> BlockPyFunction<TypedCodegenModuleShape> {
    CodegenToTyped.map_fn(function)
}

pub fn lower_typed_function_if_tests_to_truthy(
    mut function: BlockPyFunction<TypedCodegenModuleShape>,
) -> BlockPyFunction<TypedCodegenModuleShape> {
    for block in &mut function.blocks {
        if let crate::block_py::BlockTerm::IfTerm(if_term) = &mut block.term {
            if matches!(if_term.test, InstrTyped::Truthy(_)) {
                continue;
            }
            let old_test = std::mem::replace(&mut if_term.test, InstrTyped::constant_none());
            let meta = old_test.meta();
            if_term.test = InstrTyped::Truthy(TypedTruthy::new(old_test).with_meta(meta));
        }
    }
    function
}

pub fn lower_typed_if_tests_to_truthy(
    mut module: BlockPyModule<TypedCodegenModuleShape>,
) -> BlockPyModule<TypedCodegenModuleShape> {
    module.callable_defs = module
        .callable_defs
        .into_iter()
        .map(lower_typed_function_if_tests_to_truthy)
        .collect();
    module
}

struct TypedToCodegen;

impl TryMapInstr<InstrTyped, InstrCodegen, String> for TypedToCodegen {
    fn try_map_instr(&mut self, instr: InstrTyped) -> Result<InstrCodegen, String> {
        Ok(match instr {
            InstrTyped::Truthy(_) => {
                return Err(
                    "typed truthiness instruction requires typed codegen emission".to_string(),
                );
            }
            InstrTyped::Load(op) => InstrCodegenOp::Load(op.try_map_children(self)?),
            InstrTyped::BinOp(op) => InstrCodegenOp::BinOp(op.try_map_children(self)?),
            InstrTyped::LegacyTuple(op) => InstrCodegenOp::Tuple(op.try_map_children(self)?),
            InstrTyped::LegacyUnaryOp(op) => InstrCodegenOp::UnaryOp(op.try_map_children(self)?),
            InstrTyped::LegacyCalleeFunctionId(op) => {
                InstrCodegenOp::CalleeFunctionId(op.try_map_children(self)?)
            }
            InstrTyped::LegacyCall(op) => InstrCodegenOp::Call(op.try_map_children(self)?),
            InstrTyped::LegacyCallDirect(op) => {
                InstrCodegenOp::CallDirect(op.try_map_children(self)?)
            }
            InstrTyped::LegacyGetAttr(op) => InstrCodegenOp::GetAttr(op.try_map_children(self)?),
            InstrTyped::LegacySetAttr(op) => InstrCodegenOp::SetAttr(op.try_map_children(self)?),
            InstrTyped::LegacyGetItem(op) => InstrCodegenOp::GetItem(op.try_map_children(self)?),
            InstrTyped::LegacySetItem(op) => InstrCodegenOp::SetItem(op.try_map_children(self)?),
            InstrTyped::LegacyDelItem(op) => InstrCodegenOp::DelItem(op.try_map_children(self)?),
            InstrTyped::LegacyStore(op) => InstrCodegenOp::Store(op.try_map_children(self)?),
            InstrTyped::LegacyDel(op) => InstrCodegenOp::Del(op.try_map_children(self)?),
            InstrTyped::LegacyMakeCell(op) => InstrCodegenOp::MakeCell(op.try_map_children(self)?),
            InstrTyped::LegacyIncrementCounter(op) => InstrCodegenOp::IncrementCounter(op),
            InstrTyped::LegacyCellRef(op) => InstrCodegenOp::CellRef(op),
            InstrTyped::LegacyMakeFunctionWithClosure(op) => {
                InstrCodegenOp::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
        })
    }

    fn try_map_name(&mut self, name: ResolvedName) -> Result<ResolvedName, String> {
        Ok(name)
    }
}

pub fn try_lower_typed_instr_to_codegen_legacy(instr: InstrTyped) -> Result<InstrCodegen, String> {
    TypedToCodegen.try_map_instr(instr)
}

pub fn try_lower_typed_term_to_codegen_legacy(
    term: BlockTerm<InstrTyped>,
) -> Result<BlockTerm<InstrCodegen>, String> {
    TypedToCodegen.try_map_term(term)
}

pub fn try_lower_typed_module_to_codegen_legacy(
    module: BlockPyModule<TypedCodegenModuleShape>,
) -> Result<BlockPyModule<CodegenModuleShape>, String> {
    TypedToCodegen.try_map_module(module)
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
    CellRefForName(CellRefForName),
    CellRef(CellRef),
    MakeFunction(#[rkyv(omit_bounds)] MakeFunction<Self>),
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
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
    CellRef(CellRef),
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
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

#[derive(Debug, Clone)]
pub struct TypedCodegenModuleShape;

impl ModuleShape for TypedCodegenModuleShape {
    type Instr = InstrTyped;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenUnidentifiedModuleShape;

impl ModuleShape for CodegenUnidentifiedModuleShape {
    type Instr = InstrCodegen;
}

pub(crate) use blockpy_generators::lower_yield_in_lowered_core_blockpy_module_bundle;
pub use blockpy_to_bb::{lower_try_jump_exception_flow, normalize_bb_module_strings};
pub use escape_analysis::{
    summarize_module_escapes, ConstructorFieldAccess, ConstructorFieldStore, ConstructorFieldValue,
    EscapeSummaryModule, FieldInitializerConstructorSummary, FunctionEscapeSummary,
    NonEscapingConstructorAllocationSummary, NonEscapingConstructorSummary,
};
pub use inline_plan::{
    plan_module_inlining, FunctionInlinePlan, InlinePlanModule, StraightlineConstructorInlinePlan,
};
pub use inline_sites::{
    collect_inline_call_sites, InlineCallSiteModule, StraightlineConstructorCallSite,
};
pub use inline_transform::{
    bind_simple_direct_call_inline_args, build_single_block_inline_fragment,
    build_single_block_inline_fragment_to_target, build_single_block_inline_fragment_with_bindings,
    inline_simple_direct_call_stores, InlineFragment, InlineLocal, InlineRewriteStats,
    InlineUnsupportedReason, InlineValueBindings,
};
pub use instr_id::{
    assign_function_instr_ids, assign_module_instr_ids, reassign_codegen_function_instr_ids,
    reassign_codegen_module_instr_ids, validate_codegen_instr_ids,
};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
pub use local_env_plan::{
    plan_function_local_env_resume, plan_function_locals, plan_local_env_module,
    plan_local_env_resume_module, render_local_env_function_plan, render_local_env_module_plan,
    render_local_env_resume_function_plan, render_local_env_resume_module_plan,
    render_planned_local_binding, validate_local_env_module_plan,
    validate_local_env_resume_module_plan, BlockLocalPlan, BlockParamFacts,
    FunctionLocalEnvResumePlan, FunctionLocalPlan, LocalEnvModulePlan, LocalEnvResumeBinding,
    LocalEnvResumeBindingState, LocalEnvResumeEntry, LocalEnvResumeModulePlan, LocalEnvResumePoint,
    LocalEnvResumeStatePrecision, LocalEnvResumeValueSource, LocalRefKind, ParamBindingFacts,
    ParamProvenance, PlannedLocalBinding, PlannedLocalStorage,
};
pub use ownership_effects::{
    compute_function_local_live_ins, compute_function_local_must_bound_ins, plan_ownership_effects,
    validate_ownership_effects, BlockRefcountPlan, FunctionRefcountPlan, LocalRefState,
    RefcountAction, RefcountActionKind, RefcountLocal, RefcountPlan, RefcountReleaseReason,
    RefcountSite,
};
pub use trace::{
    define_bb_module_deopt_entry_counters, deopt_entry_counter_instrumentation_enabled,
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
    refcount_counter_instrumentation_enabled,
};

pub fn relabel_dense_bb_module<P: ModuleShape>(module: &mut BlockPyModule<P>) {
    for callable in &mut module.callable_defs {
        relabel_blockpy_blocks_dense(&mut callable.blocks);
    }
}

#[cfg(test)]
mod typed_codegen_tests {
    use super::*;
    use crate::block_py::{ChildVisitable, Visit};

    #[derive(Default)]
    struct LegacyInstrCounter {
        total: usize,
        truthy: usize,
        loads: usize,
        binops: usize,
        non_legacy: usize,
    }

    impl Visit<InstrTyped> for LegacyInstrCounter {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.total += 1;
            if matches!(expr, InstrTyped::Truthy(_)) {
                self.truthy += 1;
            }
            if matches!(expr, InstrTyped::Load(_)) {
                self.loads += 1;
            }
            if !expr.is_legacy() {
                self.non_legacy += 1;
            }
            if matches!(expr, InstrTyped::BinOp(_)) {
                self.binops += 1;
            }
            expr.visit_children(self);
        }
    }

    #[derive(Default, Eq, PartialEq, Debug)]
    struct CodegenInstrCounter {
        total: usize,
        binops: usize,
        calls: usize,
    }

    impl Visit<InstrCodegen> for CodegenInstrCounter {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            self.total += 1;
            match expr {
                InstrCodegen::BinOp(_) => self.binops += 1,
                InstrCodegen::Call(_) => self.calls += 1,
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    fn count_codegen_instrs(module: &BlockPyModule<CodegenModuleShape>) -> CodegenInstrCounter {
        let mut counter = CodegenInstrCounter::default();
        for function in &module.callable_defs {
            for block in &function.blocks {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }
        counter
    }

    #[test]
    fn lower_codegen_module_to_typed_keeps_loads_first_class() {
        let lowered =
            crate::lower_python_to_blockpy_for_testing("def f(a, b):\n    return a + b\n")
                .expect("source should lower");
        let function_count = lowered.codegen_module.callable_defs.len();
        let global_names = lowered.codegen_module.global_names.clone();

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        assert_eq!(typed.callable_defs.len(), function_count);
        assert_eq!(typed.global_names, global_names);

        let mut counter = LegacyInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }

        assert!(counter.total > 0);
        assert!(counter.binops > 0);
        assert!(counter.loads > 0);
        assert_eq!(counter.truthy, 0);
        assert_eq!(counter.non_legacy, counter.loads + counter.binops);
    }

    #[test]
    fn typed_legacy_module_round_trips_to_codegen_shape() {
        let lowered =
            crate::lower_python_to_blockpy_for_testing("def f(a, b):\n    return g(a + b)\n")
                .expect("source should lower");
        let original_counts = count_codegen_instrs(&lowered.codegen_module);

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let round_tripped = try_lower_typed_module_to_codegen_legacy(typed)
            .expect("legacy typed module should map");

        assert_eq!(count_codegen_instrs(&round_tripped), original_counts);
        assert!(original_counts.binops > 0);
        assert!(original_counts.calls > 0);
    }

    #[test]
    fn lower_typed_if_tests_to_truthy_wraps_branch_conditions() {
        let lowered = crate::lower_python_to_blockpy_for_testing(
            "def f(x):\n    if x:\n        return 1\n    return 0\n",
        )
        .expect("source should lower");
        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let typed = lower_typed_if_tests_to_truthy(typed);

        let mut counter = LegacyInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                if let crate::block_py::BlockTerm::IfTerm(if_term) = &block.term {
                    assert!(
                        matches!(if_term.test, InstrTyped::Truthy(_)),
                        "typed if test should be wrapped in an explicit truthiness op"
                    );
                    assert!(
                        try_lower_typed_term_to_codegen_legacy(block.term.clone()).is_err(),
                        "typed truthiness terms should require typed term emission"
                    );
                }
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }

        assert!(counter.truthy > 0);
        assert!(counter.loads > 0);
        assert_eq!(
            counter.non_legacy,
            counter.truthy + counter.loads + counter.binops
        );
        assert!(
            try_lower_typed_module_to_codegen_legacy(typed).is_err(),
            "typed truthiness should not silently lower through the legacy adapter"
        );
    }
}

#[cfg(test)]
mod test;
