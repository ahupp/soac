pub(crate) mod ast_symbol_analysis;
pub(crate) mod ast_to_ast;
pub(crate) mod ast_to_instr;
pub(crate) mod blockpy_expr_simplify;
mod blockpy_generators;
mod blockpy_to_bb;
pub(crate) mod core_await_lower;
mod direct_call_transform;
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
pub(crate) mod ruff_to_blockpy;
mod trace;
mod value_facts;

use crate::block_py::{
    cfg::relabel_blockpy_blocks_dense, define_instr, define_ruff_instr, runtime_name_load, Await,
    BinOp, BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgKeyword, CallArgPositional,
    CallDirect, CalleeFunctionId, CellRef, CellRefForName, ChildVisitable, Del, DelItem,
    ExprAttribute, ExprBoolOp, ExprBooleanLiteral, ExprBytesLiteral, ExprCompare, ExprDict,
    ExprDictComp, ExprEllipsisLiteral, ExprFString, ExprGenerator, ExprIf, ExprIpyEscapeCommand,
    ExprLambda, ExprList, ExprListComp, ExprName, ExprNamed, ExprNoneLiteral, ExprNumberLiteral,
    ExprSet, ExprSetComp, ExprSlice, ExprStarred, ExprStringLiteral, ExprSubscript, ExprTString,
    ExprTuple, GetAttr, GetItem, HasMeta, IncrementCounter, Instr, InstrKey, InstrWithConstantNone,
    LiteralValue, Load, LocalLocation, MakeCell, MakeFunction, MakeFunctionWithClosure,
    MapFunction, MapInstr, MapModule, Mappable, Meta, ModuleShape, NameLike, NameLocation,
    PrettyPrint, PrettyPrinter, ResolvedName, RuntimeFunctionId, RuntimeName, SetAttr, SetItem,
    StmtAnnAssign, StmtAssert, StmtAssign, StmtAugAssign, StmtBreak, StmtClassDef, StmtContinue,
    StmtDelete, StmtExpr, StmtFor, StmtFunctionDef, StmtGlobal, StmtIf, StmtImport, StmtImportFrom,
    StmtIpyEscapeCommand, StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn, StmtTry,
    StmtTypeAlias, StmtWhile, StmtWith, Store, TryMapInstr, TryMapModule, TryMapTerm, Tuple,
    UnaryOp, UnresolvedName, Visit, VisitMut, WithMeta, Yield, YieldFrom,
};
use ruff_python_ast::{self as ast};
use soac_macros::{enum_broadcast, DelegateMatchDefault};

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
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
    type ModuleConstant = InstrResolved;
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

#[derive(Clone, derive_more::From, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(serialize_bounds(
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(bounds(
    __C: rkyv::validation::ArchiveContext,
)))]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub enum InstrCodegenOp {
    BinOp(#[rkyv(omit_bounds)] BinOp<Self>),
    UnaryOp(#[rkyv(omit_bounds)] UnaryOp<Self>),
    CalleeFunctionId(#[rkyv(omit_bounds)] CalleeFunctionId<Self>),
    DirectFunctionIdGuardTest(#[rkyv(omit_bounds)] DirectFunctionIdGuardTest<Self>),
    DirectReceiverTypeVersionGuardTest(
        #[rkyv(omit_bounds)] DirectReceiverTypeVersionGuardTest<Self>,
    ),
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
    type Extra = ();
}

define_instr! {
    pub struct DirectFunctionIdGuardTest<E> {
        value: Box<E>,
        function_id: RuntimeFunctionId,
    }
}

define_instr! {
    pub struct DirectReceiverTypeVersionGuardTest<E> {
        value: Box<E>,
        owner_type_ref: TypedAttrOwnerRef,
        type_version: u32,
    }
}

define_ruff_instr! {
    pub struct TypedTruthy<E> {
        value: Box<E>,
    }
}

impl<E: Instr> TypedTruthy<E> {
    pub fn value(&self) -> &E {
        &self.value
    }

    pub fn into_value(self) -> E {
        *self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedDirectCallArgSource {
    Provided(usize),
    DefaultSentinel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedDirectCallArgPlan {
    pub sources: Vec<TypedDirectCallArgSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedDirectFunctionCallGuard {
    pub function_id: RuntimeFunctionId,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedDirectMethodCallGuard {
    pub function_id: RuntimeFunctionId,
    pub owner_type_ref: TypedAttrOwnerRef,
    pub type_version: u32,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedDirectConstructorCallGuard {
    pub function_id: RuntimeFunctionId,
    pub owner_type_ref: TypedAttrOwnerRef,
    pub type_version: u32,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedDirectCallableCallGuard {
    Function(TypedDirectFunctionCallGuard),
    Constructor(TypedDirectConstructorCallGuard),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedCallAccessPlan {
    Generic,
    ProfiledCallableTargets {
        targets: Vec<RuntimeFunctionId>,
    },
    ProfiledMethodTargets {
        targets: Vec<RuntimeFunctionId>,
    },
    GuardedCallable {
        function_guards: Vec<TypedDirectFunctionCallGuard>,
        constructor_guards: Vec<TypedDirectConstructorCallGuard>,
    },
    GuardedMethod {
        method_name: String,
        method_guards: Vec<TypedDirectMethodCallGuard>,
    },
    GuardedRuntimeProtocolMethod {
        runtime_name: RuntimeName,
        method_name: String,
        method_guards: Vec<TypedDirectMethodCallGuard>,
    },
}

#[derive(Clone)]
pub struct TypedCall<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub func: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
    pub access: TypedCallAccessPlan,
}

impl<E: Instr> TypedCall<E> {
    pub fn generic(
        func: impl Into<Box<E>>,
        args: impl Into<Vec<CallArgPositional<E>>>,
        keywords: impl Into<Vec<CallArgKeyword<E>>>,
    ) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            func: func.into(),
            args: args.into(),
            keywords: keywords.into(),
            access: TypedCallAccessPlan::Generic,
        }
    }

    pub fn from_legacy(op: Call<E>) -> Self {
        Self {
            _meta: op.meta(),
            extra: op.extra,
            func: op.func,
            args: op.args,
            keywords: op.keywords,
            access: TypedCallAccessPlan::Generic,
        }
    }

    pub fn into_legacy(self) -> Call<E> {
        Call::new(self.func, self.args, self.keywords)
            .with_extra(self.extra)
            .with_meta(self._meta)
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedCall<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedCall")
            .field("func", &self.func)
            .field("args", &self.args)
            .field("keywords", &self.keywords)
            .field("access", &self.access)
            .finish()
    }
}

impl<E: Instr> HasMeta for TypedCall<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for TypedCall<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for TypedCall<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.func);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
        for keyword in &self.keywords {
            visitor.visit_instr(keyword.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.func);
        for arg in &mut self.args {
            visitor.visit_instr_mut(arg.expr_mut());
        }
        for keyword in &mut self.keywords {
            visitor.visit_instr_mut(keyword.expr_mut());
        }
    }
}

impl<E: Instr> Mappable<E> for TypedCall<E> {
    type Mapped<T: Instr> = TypedCall<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        TypedCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.map_instr(*self.func)),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            keywords: self
                .keywords
                .into_iter()
                .map(|keyword| keyword.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            access: self.access,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(TypedCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.try_map_instr(*self.func)?),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            keywords: self
                .keywords
                .into_iter()
                .map(|keyword| keyword.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            access: self.access,
        })
    }
}

#[derive(Clone)]
pub struct TypedDirectCallableCall<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub func: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub guard: TypedDirectCallableCallGuard,
}

impl<E: Instr> TypedDirectCallableCall<E> {
    pub fn new(
        func: impl Into<Box<E>>,
        args: impl Into<Vec<CallArgPositional<E>>>,
        guard: TypedDirectCallableCallGuard,
    ) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            func: func.into(),
            args: args.into(),
            guard,
        }
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedDirectCallableCall<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedDirectCallableCall")
            .field("func", &self.func)
            .field("args", &self.args)
            .field("guard", &self.guard)
            .finish()
    }
}

impl<E: Instr> HasMeta for TypedDirectCallableCall<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for TypedDirectCallableCall<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for TypedDirectCallableCall<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.func);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.func);
        for arg in &mut self.args {
            visitor.visit_instr_mut(arg.expr_mut());
        }
    }
}

impl<E: Instr> Mappable<E> for TypedDirectCallableCall<E> {
    type Mapped<T: Instr> = TypedDirectCallableCall<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        TypedDirectCallableCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.map_instr(*self.func)),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            guard: self.guard,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(TypedDirectCallableCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.try_map_instr(*self.func)?),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            guard: self.guard,
        })
    }
}

#[derive(Clone)]
pub struct TypedGuardedCallableCall<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub func: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
    pub function_guards: Vec<TypedDirectFunctionCallGuard>,
    pub constructor_guards: Vec<TypedDirectConstructorCallGuard>,
}

impl<E: Instr> TypedGuardedCallableCall<E> {
    pub fn from_typed_call(
        call: TypedCall<E>,
        function_guards: Vec<TypedDirectFunctionCallGuard>,
        constructor_guards: Vec<TypedDirectConstructorCallGuard>,
    ) -> Self {
        Self {
            _meta: call._meta,
            extra: call.extra,
            func: call.func,
            args: call.args,
            keywords: call.keywords,
            function_guards,
            constructor_guards,
        }
    }

    pub fn into_typed_call(self) -> TypedCall<E> {
        TypedCall {
            _meta: self._meta,
            extra: self.extra,
            func: self.func,
            args: self.args,
            keywords: self.keywords,
            access: TypedCallAccessPlan::GuardedCallable {
                function_guards: self.function_guards,
                constructor_guards: self.constructor_guards,
            },
        }
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedGuardedCallableCall<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedGuardedCallableCall")
            .field("func", &self.func)
            .field("args", &self.args)
            .field("keywords", &self.keywords)
            .field("function_guards", &self.function_guards)
            .field("constructor_guards", &self.constructor_guards)
            .finish()
    }
}

impl<E: Instr> HasMeta for TypedGuardedCallableCall<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for TypedGuardedCallableCall<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for TypedGuardedCallableCall<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.func);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
        for keyword in &self.keywords {
            visitor.visit_instr(keyword.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.func);
        for arg in &mut self.args {
            visitor.visit_instr_mut(arg.expr_mut());
        }
        for keyword in &mut self.keywords {
            visitor.visit_instr_mut(keyword.expr_mut());
        }
    }
}

impl<E: Instr> Mappable<E> for TypedGuardedCallableCall<E> {
    type Mapped<T: Instr> = TypedGuardedCallableCall<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        TypedGuardedCallableCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.map_instr(*self.func)),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            keywords: self
                .keywords
                .into_iter()
                .map(|keyword| keyword.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            function_guards: self.function_guards,
            constructor_guards: self.constructor_guards,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(TypedGuardedCallableCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.try_map_instr(*self.func)?),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            keywords: self
                .keywords
                .into_iter()
                .map(|keyword| keyword.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            function_guards: self.function_guards,
            constructor_guards: self.constructor_guards,
        })
    }
}

#[derive(Clone)]
pub struct TypedGuardedMethodCall<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub func: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
    pub method_name: String,
    pub method_guards: Vec<TypedDirectMethodCallGuard>,
}

impl<E: Instr> TypedGuardedMethodCall<E> {
    pub fn from_typed_call(
        call: TypedCall<E>,
        method_name: String,
        method_guards: Vec<TypedDirectMethodCallGuard>,
    ) -> Self {
        Self {
            _meta: call._meta,
            extra: call.extra,
            func: call.func,
            args: call.args,
            keywords: call.keywords,
            method_name,
            method_guards,
        }
    }

    pub fn into_typed_call(self) -> TypedCall<E> {
        TypedCall {
            _meta: self._meta,
            extra: self.extra,
            func: self.func,
            args: self.args,
            keywords: self.keywords,
            access: TypedCallAccessPlan::GuardedMethod {
                method_name: self.method_name,
                method_guards: self.method_guards,
            },
        }
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedGuardedMethodCall<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedGuardedMethodCall")
            .field("func", &self.func)
            .field("args", &self.args)
            .field("keywords", &self.keywords)
            .field("method_name", &self.method_name)
            .field("method_guards", &self.method_guards)
            .finish()
    }
}

impl<E: Instr> HasMeta for TypedGuardedMethodCall<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for TypedGuardedMethodCall<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for TypedGuardedMethodCall<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.func);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
        for keyword in &self.keywords {
            visitor.visit_instr(keyword.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.func);
        for arg in &mut self.args {
            visitor.visit_instr_mut(arg.expr_mut());
        }
        for keyword in &mut self.keywords {
            visitor.visit_instr_mut(keyword.expr_mut());
        }
    }
}

impl<E: Instr> Mappable<E> for TypedGuardedMethodCall<E> {
    type Mapped<T: Instr> = TypedGuardedMethodCall<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        TypedGuardedMethodCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.map_instr(*self.func)),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            keywords: self
                .keywords
                .into_iter()
                .map(|keyword| keyword.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            method_name: self.method_name,
            method_guards: self.method_guards,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(TypedGuardedMethodCall {
            _meta: self._meta,
            extra: Default::default(),
            func: Box::new(map.try_map_instr(*self.func)?),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            keywords: self
                .keywords
                .into_iter()
                .map(|keyword| keyword.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            method_name: self.method_name,
            method_guards: self.method_guards,
        })
    }
}

#[derive(Clone)]
pub struct TypedDirectMethodCall<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub receiver: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub method_name: String,
    pub guard: TypedDirectMethodCallGuard,
}

impl<E: Instr> TypedDirectMethodCall<E> {
    pub fn new(
        receiver: impl Into<Box<E>>,
        args: impl Into<Vec<CallArgPositional<E>>>,
        method_name: impl Into<String>,
        guard: TypedDirectMethodCallGuard,
    ) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            receiver: receiver.into(),
            args: args.into(),
            method_name: method_name.into(),
            guard,
        }
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedDirectMethodCall<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedDirectMethodCall")
            .field("receiver", &self.receiver)
            .field("args", &self.args)
            .field("method_name", &self.method_name)
            .field("guard", &self.guard)
            .finish()
    }
}

impl<E: Instr> HasMeta for TypedDirectMethodCall<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for TypedDirectMethodCall<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for TypedDirectMethodCall<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.receiver);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.receiver);
        for arg in &mut self.args {
            visitor.visit_instr_mut(arg.expr_mut());
        }
    }
}

impl<E: Instr> Mappable<E> for TypedDirectMethodCall<E> {
    type Mapped<T: Instr> = TypedDirectMethodCall<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        TypedDirectMethodCall {
            _meta: self._meta,
            extra: Default::default(),
            receiver: Box::new(map.map_instr(*self.receiver)),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            method_name: self.method_name,
            guard: self.guard,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(TypedDirectMethodCall {
            _meta: self._meta,
            extra: Default::default(),
            receiver: Box::new(map.try_map_instr(*self.receiver)?),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.try_map_instr(|expr| map.try_map_instr(expr)))
                .collect::<Result<Vec<_>, _>>()?,
            method_name: self.method_name,
            guard: self.guard,
        })
    }
}

impl<E> PrettyPrint for TypedCall<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, "TypedCall { func: ")?;
        self.func.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", args: ")?;
        self.args.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", keywords: ")?;
        self.keywords.fmt_pretty(printer)?;
        std::fmt::Write::write_fmt(printer, format_args!(", access: {:?} }}", self.access))
    }
}

impl<E> PrettyPrint for TypedDirectCallableCall<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, "TypedDirectCallableCall { func: ")?;
        self.func.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", args: ")?;
        self.args.fmt_pretty(printer)?;
        std::fmt::Write::write_fmt(printer, format_args!(", guard: {:?} }}", self.guard))
    }
}

impl<E> PrettyPrint for TypedGuardedCallableCall<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, "TypedGuardedCallableCall { func: ")?;
        self.func.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", args: ")?;
        self.args.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", keywords: ")?;
        self.keywords.fmt_pretty(printer)?;
        std::fmt::Write::write_fmt(
            printer,
            format_args!(
                ", function_guards: {:?}, constructor_guards: {:?} }}",
                self.function_guards, self.constructor_guards
            ),
        )
    }
}

impl<E> PrettyPrint for TypedGuardedMethodCall<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, "TypedGuardedMethodCall { func: ")?;
        self.func.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", args: ")?;
        self.args.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", keywords: ")?;
        self.keywords.fmt_pretty(printer)?;
        std::fmt::Write::write_fmt(
            printer,
            format_args!(
                ", method_name: {:?}, method_guards: {:?} }}",
                self.method_name, self.method_guards
            ),
        )
    }
}

impl<E> PrettyPrint for TypedDirectMethodCall<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, "TypedDirectMethodCall { receiver: ")?;
        self.receiver.fmt_pretty(printer)?;
        std::fmt::Write::write_str(printer, ", args: ")?;
        self.args.fmt_pretty(printer)?;
        std::fmt::Write::write_fmt(
            printer,
            format_args!(
                ", method_name: {:?}, guard: {:?} }}",
                self.method_name, self.guard
            ),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedDirectCallGuardTestKind {
    RuntimeFunctionId {
        function_id: RuntimeFunctionId,
    },
    ExactCallableTypeVersion {
        owner_type_ref: TypedAttrOwnerRef,
        type_version: u32,
    },
    ExactReceiverTypeVersion {
        owner_type_ref: TypedAttrOwnerRef,
        type_version: u32,
    },
}

define_ruff_instr! {
    pub struct TypedDirectCallGuardTest<E> {
        value: Box<E>,
        kind: TypedDirectCallGuardTestKind,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum TypedAttrOwnerRef {
    CpythonTypeSymbol(String),
    TypeKey {
        module_name: String,
        qualname: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedIndexedFieldGuard {
    pub expected_index: u32,
    pub owner_type_ref: TypedAttrOwnerRef,
    pub type_version: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedIndexedFieldPlanSource {
    LegacyProfile,
    OptimizationPlanV3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedAttrAccessPlan {
    Generic,
    IndexedField {
        source: TypedIndexedFieldPlanSource,
        guards: Vec<TypedIndexedFieldGuard>,
    },
}

define_ruff_instr! {
    pub struct TypedGetAttr<E> {
        value: Box<E>,
        attr: Box<E>,
        access: TypedAttrAccessPlan,
    }
}

impl<E: Instr> TypedGetAttr<E> {
    pub fn generic(value: impl Into<Box<E>>, attr: impl Into<Box<E>>) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            value: value.into(),
            attr: attr.into(),
            access: TypedAttrAccessPlan::Generic,
        }
    }

    pub fn from_legacy(op: GetAttr<E>) -> Self {
        Self {
            _meta: op.meta(),
            extra: op.extra,
            value: op.value,
            attr: op.attr,
            access: TypedAttrAccessPlan::Generic,
        }
    }

    pub fn into_legacy(self) -> GetAttr<E> {
        GetAttr::new(self.value, self.attr)
            .with_extra(self.extra)
            .with_meta(self._meta)
    }
}

define_ruff_instr! {
    pub struct TypedSetAttr<E> {
        value: Box<E>,
        attr: Box<E>,
        replacement: Box<E>,
        access: TypedAttrAccessPlan,
    }
}

impl<E: Instr> TypedSetAttr<E> {
    pub fn generic(
        value: impl Into<Box<E>>,
        attr: impl Into<Box<E>>,
        replacement: impl Into<Box<E>>,
    ) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            value: value.into(),
            attr: attr.into(),
            replacement: replacement.into(),
            access: TypedAttrAccessPlan::Generic,
        }
    }

    pub fn from_legacy(op: SetAttr<E>) -> Self {
        Self {
            _meta: op.meta(),
            extra: op.extra,
            value: op.value,
            attr: op.attr,
            replacement: op.replacement,
            access: TypedAttrAccessPlan::Generic,
        }
    }

    pub fn into_legacy(self) -> SetAttr<E> {
        SetAttr::new(self.value, self.attr, self.replacement)
            .with_extra(self.extra)
            .with_meta(self._meta)
    }
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
pub enum InstrTyped {
    Truthy(TypedTruthy<Self>),
    Load(Load<Self>),
    BinOp(BinOp<Self>),
    LegacyTuple(Tuple<Self>),
    LegacyUnaryOp(UnaryOp<Self>),
    LegacyCalleeFunctionId(CalleeFunctionId<Self>),
    CallTyped(TypedCall<Self>),
    GuardedCallableCallTyped(TypedGuardedCallableCall<Self>),
    GuardedMethodCallTyped(TypedGuardedMethodCall<Self>),
    DirectCallableCallTyped(TypedDirectCallableCall<Self>),
    DirectMethodCallTyped(TypedDirectMethodCall<Self>),
    DirectCallGuardTest(TypedDirectCallGuardTest<Self>),
    LegacyCall(Call<Self>),
    LegacyCallDirect(CallDirect<Self>),
    GetAttrTyped(TypedGetAttr<Self>),
    SetAttrTyped(TypedSetAttr<Self>),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedResultDemand {
    EffectOnly,
    PyObject { borrowed_ok: bool },
    I32Bool01,
    I64,
    I64Index,
}

impl TypedResultDemand {
    pub const PYOBJECT_OWNED: Self = Self::PyObject { borrowed_ok: false };
    pub const PYOBJECT_BORROWED_OK: Self = Self::PyObject { borrowed_ok: true };
    pub const I32_BOOL01: Self = Self::I32Bool01;
    pub const I64_VALUE: Self = Self::I64;
    pub const I64_INDEX: Self = Self::I64Index;

    pub const fn needs_value(self) -> bool {
        matches!(
            self,
            Self::PyObject { .. } | Self::I32Bool01 | Self::I64 | Self::I64Index
        )
    }

    pub const fn borrowed_ok(self) -> bool {
        match self {
            Self::EffectOnly | Self::I32Bool01 | Self::I64 | Self::I64Index => false,
            Self::PyObject { borrowed_ok } => borrowed_ok,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedPyObjectOwnershipPlan {
    Owned,
    BorrowedLocal,
    Immortal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedPlannedResult {
    EffectOnly,
    PyObject {
        ownership: TypedPyObjectOwnershipPlan,
    },
    I32Bool01,
    I64,
}

impl TypedPlannedResult {
    pub const PYOBJECT_OWNED: Self = Self::PyObject {
        ownership: TypedPyObjectOwnershipPlan::Owned,
    };
    pub const PYOBJECT_BORROWED_LOCAL: Self = Self::PyObject {
        ownership: TypedPyObjectOwnershipPlan::BorrowedLocal,
    };
    pub const PYOBJECT_IMMORTAL: Self = Self::PyObject {
        ownership: TypedPyObjectOwnershipPlan::Immortal,
    };
    pub const I32_BOOL01: Self = Self::I32Bool01;
    pub const I64_VALUE: Self = Self::I64;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypedInstrExtra {
    pub result_facts: Option<ValueFacts>,
    pub demand: Option<TypedResultDemand>,
    pub planned_result: Option<TypedPlannedResult>,
}

impl TypedInstrExtra {
    pub fn result_facts(&self) -> Option<ValueFacts> {
        self.result_facts
    }

    pub fn refine_result_facts(&mut self, facts: ValueFacts) -> bool {
        if self.result_facts == Some(facts) {
            return false;
        }
        self.result_facts = Some(facts);
        true
    }

    pub fn clear_result_facts(&mut self) -> bool {
        self.result_facts.take().is_some()
    }

    pub fn demand(&self) -> Option<TypedResultDemand> {
        self.demand
    }

    pub fn set_demand(&mut self, demand: TypedResultDemand) -> bool {
        if self.demand == Some(demand) {
            return false;
        }
        self.demand = Some(demand);
        true
    }

    pub fn clear_demand(&mut self) -> bool {
        self.demand.take().is_some()
    }

    pub fn planned_result(&self) -> Option<TypedPlannedResult> {
        self.planned_result
    }

    pub fn set_planned_result(&mut self, planned_result: TypedPlannedResult) -> bool {
        if self.planned_result == Some(planned_result) {
            return false;
        }
        self.planned_result = Some(planned_result);
        true
    }

    pub fn clear_planned_result(&mut self) -> bool {
        self.planned_result.take().is_some()
    }
}

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

    pub fn typed_extra(&self) -> Option<&TypedInstrExtra> {
        match self {
            Self::Truthy(op) => Some(&op.extra),
            Self::Load(op) => Some(op.extra()),
            Self::BinOp(op) => Some(op.extra()),
            Self::LegacyTuple(op) => Some(op.extra()),
            Self::LegacyUnaryOp(op) => Some(op.extra()),
            Self::LegacyCalleeFunctionId(op) => Some(op.extra()),
            Self::CallTyped(op) => Some(&op.extra),
            Self::GuardedCallableCallTyped(op) => Some(&op.extra),
            Self::GuardedMethodCallTyped(op) => Some(&op.extra),
            Self::DirectCallableCallTyped(op) => Some(&op.extra),
            Self::DirectMethodCallTyped(op) => Some(&op.extra),
            Self::DirectCallGuardTest(op) => Some(&op.extra),
            Self::LegacyCall(op) => Some(op.extra()),
            Self::LegacyCallDirect(op) => Some(op.extra()),
            Self::GetAttrTyped(op) => Some(&op.extra),
            Self::SetAttrTyped(op) => Some(&op.extra),
            Self::LegacyGetAttr(op) => Some(op.extra()),
            Self::LegacySetAttr(op) => Some(op.extra()),
            Self::LegacyGetItem(op) => Some(op.extra()),
            Self::LegacySetItem(op) => Some(op.extra()),
            Self::LegacyDelItem(op) => Some(op.extra()),
            Self::LegacyStore(op) => Some(op.extra()),
            Self::LegacyDel(op) => Some(op.extra()),
            Self::LegacyMakeCell(op) => Some(op.extra()),
            Self::LegacyMakeFunctionWithClosure(op) => Some(op.extra()),
            Self::LegacyIncrementCounter(_) | Self::LegacyCellRef(_) => None,
        }
    }

    pub fn typed_extra_mut(&mut self) -> Option<&mut TypedInstrExtra> {
        match self {
            Self::Truthy(op) => Some(&mut op.extra),
            Self::Load(op) => Some(op.extra_mut()),
            Self::BinOp(op) => Some(op.extra_mut()),
            Self::LegacyTuple(op) => Some(op.extra_mut()),
            Self::LegacyUnaryOp(op) => Some(op.extra_mut()),
            Self::LegacyCalleeFunctionId(op) => Some(op.extra_mut()),
            Self::CallTyped(op) => Some(&mut op.extra),
            Self::GuardedCallableCallTyped(op) => Some(&mut op.extra),
            Self::GuardedMethodCallTyped(op) => Some(&mut op.extra),
            Self::DirectCallableCallTyped(op) => Some(&mut op.extra),
            Self::DirectMethodCallTyped(op) => Some(&mut op.extra),
            Self::DirectCallGuardTest(op) => Some(&mut op.extra),
            Self::LegacyCall(op) => Some(op.extra_mut()),
            Self::LegacyCallDirect(op) => Some(op.extra_mut()),
            Self::GetAttrTyped(op) => Some(&mut op.extra),
            Self::SetAttrTyped(op) => Some(&mut op.extra),
            Self::LegacyGetAttr(op) => Some(op.extra_mut()),
            Self::LegacySetAttr(op) => Some(op.extra_mut()),
            Self::LegacyGetItem(op) => Some(op.extra_mut()),
            Self::LegacySetItem(op) => Some(op.extra_mut()),
            Self::LegacyDelItem(op) => Some(op.extra_mut()),
            Self::LegacyStore(op) => Some(op.extra_mut()),
            Self::LegacyDel(op) => Some(op.extra_mut()),
            Self::LegacyMakeCell(op) => Some(op.extra_mut()),
            Self::LegacyMakeFunctionWithClosure(op) => Some(op.extra_mut()),
            Self::LegacyIncrementCounter(_) | Self::LegacyCellRef(_) => None,
        }
    }

    pub fn result_facts(&self) -> Option<ValueFacts> {
        self.typed_extra().and_then(TypedInstrExtra::result_facts)
    }

    pub fn result_demand(&self) -> Option<TypedResultDemand> {
        self.typed_extra().and_then(TypedInstrExtra::demand)
    }

    pub fn planned_result(&self) -> Option<TypedPlannedResult> {
        self.typed_extra().and_then(TypedInstrExtra::planned_result)
    }
}

impl Instr for InstrTyped {
    type Name = ResolvedName;
    type Extra = TypedInstrExtra;
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
            InstrCodegenOp::DirectFunctionIdGuardTest(op) => {
                let meta = op.meta();
                InstrTyped::DirectCallGuardTest(
                    TypedDirectCallGuardTest::new(
                        self.map_instr(*op.value),
                        TypedDirectCallGuardTestKind::RuntimeFunctionId {
                            function_id: op.function_id,
                        },
                    )
                    .with_meta(meta),
                )
            }
            InstrCodegenOp::DirectReceiverTypeVersionGuardTest(op) => {
                let meta = op.meta();
                InstrTyped::DirectCallGuardTest(
                    TypedDirectCallGuardTest::new(
                        self.map_instr(*op.value),
                        TypedDirectCallGuardTestKind::ExactReceiverTypeVersion {
                            owner_type_ref: op.owner_type_ref,
                            type_version: op.type_version,
                        },
                    )
                    .with_meta(meta),
                )
            }
            InstrCodegenOp::Call(op) => {
                InstrTyped::CallTyped(TypedCall::from_legacy(op.map_children(self)))
            }
            InstrCodegenOp::CallDirect(op) => InstrTyped::LegacyCallDirect(op.map_children(self)),
            InstrCodegenOp::GetAttr(op) => {
                InstrTyped::GetAttrTyped(TypedGetAttr::from_legacy(op.map_children(self)))
            }
            InstrCodegenOp::SetAttr(op) => {
                InstrTyped::SetAttrTyped(TypedSetAttr::from_legacy(op.map_children(self)))
            }
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

pub fn annotate_typed_module_value_facts(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
    facts: &FactStore,
) -> usize {
    module
        .callable_defs
        .iter_mut()
        .map(|function| annotate_typed_function_value_facts(function, facts))
        .sum()
}

pub fn annotate_typed_function_value_facts(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    facts: &FactStore,
) -> usize {
    struct Annotator<'a> {
        function_id: RuntimeFunctionId,
        facts: &'a FactStore,
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            let instr_id = expr.meta().instr_id;
            if let Some(instr_id) = instr_id {
                if let Some(facts) = self
                    .facts
                    .fact_for(InstrKey::new(self.function_id, instr_id))
                {
                    if let Some(extra) = expr.typed_extra_mut() {
                        self.changed += usize::from(extra.refine_result_facts(facts));
                    }
                }
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        function_id: function.function_id,
        facts,
        changed: 0,
    };
    annotator.visit_fn_mut(function);
    annotator.changed
}

fn set_typed_instr_demand(expr: &mut InstrTyped, demand: TypedResultDemand) -> usize {
    expr.typed_extra_mut()
        .map(|extra| usize::from(extra.set_demand(demand)))
        .unwrap_or(0)
}

fn set_typed_instr_planned_result(
    expr: &mut InstrTyped,
    planned_result: TypedPlannedResult,
) -> usize {
    expr.typed_extra_mut()
        .map(|extra| usize::from(extra.set_planned_result(planned_result)))
        .unwrap_or(0)
}

fn clear_typed_instr_planned_result(expr: &mut InstrTyped) -> usize {
    expr.typed_extra_mut()
        .map(|extra| usize::from(extra.clear_planned_result()))
        .unwrap_or(0)
}

fn annotate_call_arg_input_demands(
    args: &mut [CallArgPositional<InstrTyped>],
    keywords: &mut [CallArgKeyword<InstrTyped>],
) -> usize {
    let mut changed = 0;
    for arg in args {
        changed += annotate_pyobject_borrowed_input_demand(arg.expr_mut());
    }
    for keyword in keywords {
        changed += annotate_pyobject_borrowed_input_demand(keyword.expr_mut());
    }
    changed
}

fn annotate_pyobject_borrowed_input_demand(expr: &mut InstrTyped) -> usize {
    let mut changed = set_typed_instr_demand(expr, TypedResultDemand::PYOBJECT_BORROWED_OK);
    changed += annotate_typed_child_demands(expr);
    changed
}

fn annotate_typed_child_demands(expr: &mut InstrTyped) -> usize {
    match expr {
        InstrTyped::BinOp(op) => {
            annotate_pyobject_borrowed_input_demand(op.left.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.right.as_mut())
        }
        InstrTyped::LegacyUnaryOp(op) => {
            annotate_pyobject_borrowed_input_demand(op.operand.as_mut())
        }
        InstrTyped::LegacyTuple(op) => op
            .values
            .iter_mut()
            .map(annotate_pyobject_borrowed_input_demand)
            .sum(),
        InstrTyped::LegacyCalleeFunctionId(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
        }
        InstrTyped::LegacyStore(store) => {
            let mut changed =
                set_typed_instr_demand(store.value.as_mut(), TypedResultDemand::PYOBJECT_OWNED);
            changed += annotate_typed_child_demands(store.value.as_mut());
            changed
        }
        InstrTyped::CallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::GuardedCallableCallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::GuardedMethodCallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::DirectCallableCallTyped(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(call.args.as_mut_slice(), &mut []);
            changed
        }
        InstrTyped::DirectMethodCallTyped(call) => {
            let mut changed = set_typed_instr_demand(
                call.receiver.as_mut(),
                TypedResultDemand::PYOBJECT_BORROWED_OK,
            );
            changed += annotate_typed_child_demands(call.receiver.as_mut());
            changed += annotate_call_arg_input_demands(call.args.as_mut_slice(), &mut []);
            changed
        }
        InstrTyped::DirectCallGuardTest(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
        }
        InstrTyped::LegacyCall(call) => {
            let mut changed =
                set_typed_instr_demand(call.func.as_mut(), TypedResultDemand::PYOBJECT_BORROWED_OK);
            changed += annotate_typed_child_demands(call.func.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::LegacyCallDirect(call) => {
            let mut changed = set_typed_instr_demand(
                call.callable.as_mut(),
                TypedResultDemand::PYOBJECT_BORROWED_OK,
            );
            changed += annotate_typed_child_demands(call.callable.as_mut());
            changed += annotate_call_arg_input_demands(
                call.args.as_mut_slice(),
                call.keywords.as_mut_slice(),
            );
            changed
        }
        InstrTyped::GetAttrTyped(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.attr.as_mut())
        }
        InstrTyped::SetAttrTyped(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.attr.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.replacement.as_mut())
        }
        InstrTyped::LegacyGetAttr(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.attr.as_mut())
        }
        InstrTyped::LegacySetAttr(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.attr.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.replacement.as_mut())
        }
        InstrTyped::LegacyGetItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
        }
        InstrTyped::LegacySetItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.replacement.as_mut())
        }
        InstrTyped::LegacyDelItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
        }
        InstrTyped::LegacyMakeFunctionWithClosure(op) => {
            annotate_pyobject_borrowed_input_demand(op.captures.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.param_defaults.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.annotate_fn.as_mut())
        }
        _ => 0,
    }
}

pub fn annotate_typed_module_result_demands(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
) -> usize {
    module
        .callable_defs
        .iter_mut()
        .map(annotate_typed_function_result_demands)
        .sum()
}

pub fn annotate_typed_function_result_demands(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
) -> usize {
    let mut changed = 0;
    for block in &mut function.blocks {
        for expr in &mut block.body {
            changed += set_typed_instr_demand(expr, TypedResultDemand::EffectOnly);
            changed += annotate_typed_child_demands(expr);
        }
        match &mut block.term {
            BlockTerm::IfTerm(if_term) => {
                changed += set_typed_instr_demand(&mut if_term.test, TypedResultDemand::I32_BOOL01);
                changed += annotate_typed_child_demands(&mut if_term.test);
            }
            BlockTerm::BranchTable(branch) => {
                changed += set_typed_instr_demand(&mut branch.index, TypedResultDemand::I64_INDEX);
                changed += annotate_typed_child_demands(&mut branch.index);
            }
            BlockTerm::Return(value) => {
                changed += set_typed_instr_demand(value, TypedResultDemand::PYOBJECT_OWNED);
                changed += annotate_typed_child_demands(value);
            }
            BlockTerm::Raise(raise_stmt) => {
                if let Some(exc) = raise_stmt.exc.as_mut() {
                    changed += set_typed_instr_demand(exc, TypedResultDemand::PYOBJECT_OWNED);
                    changed += annotate_typed_child_demands(exc);
                }
            }
            BlockTerm::Jump(_) => {}
        }
    }
    changed
}

pub fn annotate_typed_module_planned_results(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
) -> usize {
    module
        .callable_defs
        .iter_mut()
        .map(annotate_typed_function_planned_results)
        .sum()
}

pub fn annotate_typed_function_planned_results(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
) -> usize {
    struct Annotator {
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Annotator {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            if let Some(planned_result) = plan_typed_instr_result(expr) {
                self.changed += set_typed_instr_planned_result(expr, planned_result);
            } else {
                self.changed += clear_typed_instr_planned_result(expr);
            }
        }
    }

    let mut annotator = Annotator { changed: 0 };
    annotator.visit_fn_mut(function);
    annotator.changed
}

fn plan_typed_instr_result(expr: &InstrTyped) -> Option<TypedPlannedResult> {
    let demand = expr.result_demand()?;
    Some(match demand {
        TypedResultDemand::EffectOnly => TypedPlannedResult::EffectOnly,
        TypedResultDemand::PyObject { borrowed_ok } => {
            let ownership = match expr.result_facts().and_then(ValueFacts::as_pyobj) {
                Some(py_facts) if py_facts.is_immortal() => TypedPyObjectOwnershipPlan::Immortal,
                _ if borrowed_ok && typed_instr_is_local_load(expr) => {
                    TypedPyObjectOwnershipPlan::BorrowedLocal
                }
                _ => TypedPyObjectOwnershipPlan::Owned,
            };
            TypedPlannedResult::PyObject { ownership }
        }
        TypedResultDemand::I32Bool01 => TypedPlannedResult::I32Bool01,
        TypedResultDemand::I64 | TypedResultDemand::I64Index => TypedPlannedResult::I64,
    })
}

fn typed_instr_is_local_load(expr: &InstrTyped) -> bool {
    matches!(expr, InstrTyped::Load(op) if op.name.local_location().is_some())
}

pub fn refresh_typed_function_value_facts(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
) -> usize {
    struct Refresher {
        changed: usize,
    }

    impl VisitMut<InstrTyped> for Refresher {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            let Some(facts) = infer_typed_instr_result_facts(expr) else {
                return;
            };
            if let Some(extra) = expr.typed_extra_mut() {
                self.changed += usize::from(extra.refine_result_facts(facts));
            }
        }
    }

    let mut refresher = Refresher { changed: 0 };
    refresher.visit_fn_mut(function);
    refresher.changed
}

fn infer_typed_instr_result_facts(expr: &InstrTyped) -> Option<ValueFacts> {
    match expr {
        InstrTyped::Truthy(_) => Some(ValueFacts::Bool(BoolFacts)),
        InstrTyped::Load(op) => op.extra().result_facts(),
        InstrTyped::BinOp(op) => value_facts::infer_binop_result_facts(
            op.kind,
            op.left.result_facts()?,
            op.right.result_facts()?,
        ),
        InstrTyped::LegacyUnaryOp(op) => {
            value_facts::infer_unary_result_facts(op.kind, op.operand.result_facts()?)
        }
        InstrTyped::LegacyTuple(_) => Some(ValueFacts::PyObj(PyObjFacts::known_not_none())),
        InstrTyped::CallTyped(op) => infer_typed_call_result_facts(
            op.func.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        ),
        InstrTyped::LegacyCall(op) => infer_typed_call_result_facts(
            op.func.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        ),
        InstrTyped::LegacyCallDirect(op) => infer_typed_call_result_facts(
            op.callable.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        ),
        InstrTyped::SetAttrTyped(_)
        | InstrTyped::LegacySetAttr(_)
        | InstrTyped::LegacySetItem(_)
        | InstrTyped::LegacyDelItem(_)
        | InstrTyped::LegacyDel(_) => Some(ValueFacts::PyObj(PyObjFacts::none_singleton())),
        _ => None,
    }
}

fn infer_typed_call_result_facts(
    func: &InstrTyped,
    args: &[CallArgPositional<InstrTyped>],
    keywords: &[CallArgKeyword<InstrTyped>],
) -> Option<ValueFacts> {
    if !keywords.is_empty()
        || !args
            .iter()
            .all(|arg| matches!(arg, CallArgPositional::Positional(_)))
    {
        return None;
    }
    func.result_facts()?
        .runtime_helper()
        .map(|helper| helper.signature().result)
}

pub fn validate_typed_function_value_facts(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> Result<(), String> {
    struct Validator<'a> {
        function: &'a BlockPyFunction<TypedCodegenModuleShape>,
        errors: Vec<String>,
    }

    impl Visit<InstrTyped> for Validator<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(instr_id) = expr.meta().instr_id {
                if let Some(extra) = expr.typed_extra() {
                    if extra.result_facts().is_none() {
                        self.errors.push(format!(
                            "typed instruction {} in function {} has no embedded result facts",
                            instr_id, self.function.names.qualname
                        ));
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let mut validator = Validator {
        function,
        errors: Vec::new(),
    };
    validator.visit_fn(function);
    if validator.errors.is_empty() {
        Ok(())
    } else {
        Err(validator.errors.join("; "))
    }
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
            let mut truthy = TypedTruthy::new(old_test).with_meta(meta);
            truthy
                .extra
                .refine_result_facts(ValueFacts::Bool(BoolFacts));
            if_term.test = InstrTyped::Truthy(truthy);
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

pub fn lower_typed_function_call_access_plan_instrs(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
) -> usize {
    struct Rewriter {
        count: usize,
    }

    impl VisitMut<InstrTyped> for Rewriter {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            let InstrTyped::CallTyped(call) = expr else {
                return;
            };
            let should_lower = matches!(
                call.access,
                TypedCallAccessPlan::GuardedCallable { .. }
                    | TypedCallAccessPlan::GuardedMethod { .. }
            );
            if !should_lower {
                return;
            }
            let old_expr = std::mem::replace(expr, InstrTyped::constant_none());
            let InstrTyped::CallTyped(mut call) = old_expr else {
                unreachable!("checked call shape before replacing typed instruction")
            };
            match std::mem::replace(&mut call.access, TypedCallAccessPlan::Generic) {
                TypedCallAccessPlan::GuardedCallable {
                    function_guards,
                    constructor_guards,
                } => {
                    *expr = InstrTyped::GuardedCallableCallTyped(
                        TypedGuardedCallableCall::from_typed_call(
                            call,
                            function_guards,
                            constructor_guards,
                        ),
                    );
                }
                TypedCallAccessPlan::GuardedMethod {
                    method_name,
                    method_guards,
                } => {
                    *expr = InstrTyped::GuardedMethodCallTyped(
                        TypedGuardedMethodCall::from_typed_call(call, method_name, method_guards),
                    );
                }
                _ => unreachable!("checked guarded call access before replacing typed instruction"),
            };
            self.count += 1;
        }
    }

    let mut rewriter = Rewriter { count: 0 };
    rewriter.visit_fn_mut(function);
    rewriter.count
}

pub fn validate_typed_function_call_access_plans(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> Result<(), String> {
    struct Validator {
        function_id: RuntimeFunctionId,
        error: Option<String>,
    }

    impl Visit<InstrTyped> for Validator {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.error.is_some() {
                return;
            }
            if let InstrTyped::CallTyped(call) = expr {
                if let Err(err) = validate_typed_call_access_plan(call) {
                    self.error = Some(format!(
                        "invalid typed call access plan in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::GuardedCallableCallTyped(op) = expr {
                let call = op.clone().into_typed_call();
                if let Err(err) = validate_typed_call_access_plan(&call) {
                    self.error = Some(format!(
                        "invalid typed call access plan in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::GuardedMethodCallTyped(op) = expr {
                let call = op.clone().into_typed_call();
                if let Err(err) = validate_typed_call_access_plan(&call) {
                    self.error = Some(format!(
                        "invalid typed call access plan in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::DirectCallableCallTyped(op) = expr {
                if let Err(err) = validate_typed_direct_callable_call(op) {
                    self.error = Some(format!(
                        "invalid typed direct callable call in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            if let InstrTyped::DirectMethodCallTyped(op) = expr {
                if let Err(err) = validate_typed_direct_method_call(op) {
                    self.error = Some(format!(
                        "invalid typed direct method call in function {:?}: {err}",
                        self.function_id
                    ));
                    return;
                }
            }
            expr.visit_children(self);
        }
    }

    let mut validator = Validator {
        function_id: function.function_id,
        error: None,
    };
    for block in &function.blocks {
        for instr in &block.body {
            validator.visit_instr(instr);
            if let Some(err) = validator.error.take() {
                return Err(err);
            }
        }
        validator.visit_term(&block.term);
        if let Some(err) = validator.error.take() {
            return Err(err);
        }
    }
    Ok(())
}

pub fn validate_typed_module_call_access_plans(
    module: &BlockPyModule<TypedCodegenModuleShape>,
) -> Result<(), String> {
    for function in &module.callable_defs {
        validate_typed_function_call_access_plans(function)?;
    }
    Ok(())
}

fn validate_typed_call_access_plan(call: &TypedCall<InstrTyped>) -> Result<(), String> {
    match &call.access {
        TypedCallAccessPlan::Generic
        | TypedCallAccessPlan::ProfiledCallableTargets { .. }
        | TypedCallAccessPlan::ProfiledMethodTargets { .. } => Ok(()),
        TypedCallAccessPlan::GuardedCallable {
            function_guards,
            constructor_guards,
        } => {
            validate_typed_call_simple_shape(call)?;
            for guard in function_guards {
                validate_typed_direct_call_arg_plan(call, &guard.arg_plan, 0)?;
            }
            for guard in constructor_guards {
                validate_typed_direct_call_arg_plan(call, &guard.arg_plan, 1)?;
            }
            Ok(())
        }
        TypedCallAccessPlan::GuardedMethod {
            method_name,
            method_guards,
        } => {
            validate_typed_call_simple_shape(call)?;
            if method_name.is_empty() {
                return Err("guarded method call requires a non-empty method name".to_string());
            }
            if !matches!(
                call.func.as_ref(),
                InstrTyped::GetAttrTyped(_) | InstrTyped::LegacyGetAttr(_)
            ) {
                return Err("guarded method call requires a GetAttr call target".to_string());
            }
            for guard in method_guards {
                validate_typed_direct_call_arg_plan(call, &guard.arg_plan, 1)?;
            }
            Ok(())
        }
        TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name,
            method_name,
            method_guards,
        } => {
            validate_typed_call_simple_shape(call)?;
            if *runtime_name != RuntimeName::Iter {
                return Err(format!(
                    "guarded runtime protocol call does not support runtime name {runtime_name:?}"
                ));
            }
            if method_name.is_empty() {
                return Err(
                    "guarded runtime protocol call requires a non-empty method name".to_string(),
                );
            }
            let explicit_positional_arg_count =
                validate_typed_direct_call_positional_args(call.args.as_slice())?;
            if explicit_positional_arg_count != 1 {
                return Err(format!(
                    "guarded iter protocol call requires exactly one receiver arg, got {explicit_positional_arg_count}"
                ));
            }
            for guard in method_guards {
                validate_typed_direct_call_arg_sources(&guard.arg_plan, 1)?;
            }
            Ok(())
        }
    }
}

fn validate_typed_call_simple_shape(call: &TypedCall<InstrTyped>) -> Result<(), String> {
    validate_typed_direct_call_positional_args(call.args.as_slice())?;
    if !call.keywords.is_empty() {
        return Err("guarded direct call plans do not support keyword args".to_string());
    }
    Ok(())
}

fn validate_typed_direct_callable_call(
    call: &TypedDirectCallableCall<InstrTyped>,
) -> Result<(), String> {
    let explicit_positional_arg_count =
        validate_typed_direct_call_positional_args(call.args.as_slice())?;
    match &call.guard {
        TypedDirectCallableCallGuard::Function(guard) => {
            validate_typed_direct_call_arg_sources(&guard.arg_plan, explicit_positional_arg_count)
        }
        TypedDirectCallableCallGuard::Constructor(guard) => validate_typed_direct_call_arg_sources(
            &guard.arg_plan,
            explicit_positional_arg_count + 1,
        ),
    }
}

fn validate_typed_direct_method_call(
    call: &TypedDirectMethodCall<InstrTyped>,
) -> Result<(), String> {
    if call.method_name.is_empty() {
        return Err("typed direct method call requires a non-empty method name".to_string());
    }
    let explicit_positional_arg_count =
        validate_typed_direct_call_positional_args(call.args.as_slice())?;
    validate_typed_direct_call_arg_sources(&call.guard.arg_plan, explicit_positional_arg_count + 1)
}

fn validate_typed_direct_call_positional_args(
    args: &[CallArgPositional<InstrTyped>],
) -> Result<usize, String> {
    let mut explicit_positional_arg_count = 0;
    for arg in args {
        match arg {
            CallArgPositional::Positional(_) => explicit_positional_arg_count += 1,
            CallArgPositional::Starred(_) => {
                return Err("guarded direct call plans do not support starred args".to_string());
            }
        }
    }
    Ok(explicit_positional_arg_count)
}

fn validate_typed_direct_call_arg_plan(
    call: &TypedCall<InstrTyped>,
    plan: &TypedDirectCallArgPlan,
    implicit_positional_arg_count: usize,
) -> Result<(), String> {
    let explicit_positional_arg_count =
        validate_typed_direct_call_positional_args(call.args.as_slice())?;
    let provided_positional_arg_count =
        implicit_positional_arg_count + explicit_positional_arg_count;
    validate_typed_direct_call_arg_sources(plan, provided_positional_arg_count)
}

fn validate_typed_direct_call_arg_sources(
    plan: &TypedDirectCallArgPlan,
    provided_positional_arg_count: usize,
) -> Result<(), String> {
    for source in &plan.sources {
        match source {
            TypedDirectCallArgSource::Provided(index)
                if *index >= provided_positional_arg_count =>
            {
                return Err(format!(
                    "direct call arg plan references provided arg {index}, but only {provided_positional_arg_count} args are available"
                ));
            }
            TypedDirectCallArgSource::Provided(_) | TypedDirectCallArgSource::DefaultSentinel => {}
        }
    }
    Ok(())
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
            InstrTyped::DirectCallGuardTest(op) => {
                let meta = op.meta();
                match op.kind {
                    TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id } => {
                        InstrCodegenOp::DirectFunctionIdGuardTest(
                            DirectFunctionIdGuardTest::new(
                                self.try_map_instr(*op.value)?,
                                function_id,
                            )
                            .with_meta(meta),
                        )
                    }
                    TypedDirectCallGuardTestKind::ExactReceiverTypeVersion {
                        owner_type_ref,
                        type_version,
                    } => InstrCodegenOp::DirectReceiverTypeVersionGuardTest(
                        DirectReceiverTypeVersionGuardTest::new(
                            self.try_map_instr(*op.value)?,
                            owner_type_ref,
                            type_version,
                        )
                        .with_meta(meta),
                    ),
                    TypedDirectCallGuardTestKind::ExactCallableTypeVersion { .. } => {
                        return Err(
                            "callable type-version direct-call guard test requires typed codegen emission"
                                .to_string(),
                        );
                    }
                }
            }
            InstrTyped::CallTyped(op) => {
                InstrCodegenOp::Call(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::GuardedCallableCallTyped(_) => {
                return Err(
                    "typed guarded callable call requires typed codegen emission".to_string(),
                );
            }
            InstrTyped::GuardedMethodCallTyped(_) => {
                return Err("typed guarded method call requires typed codegen emission".to_string());
            }
            InstrTyped::DirectCallableCallTyped(_) => {
                return Err(
                    "typed direct callable call requires typed codegen emission".to_string()
                );
            }
            InstrTyped::DirectMethodCallTyped(_) => {
                return Err("typed direct method call requires typed codegen emission".to_string());
            }
            InstrTyped::LegacyCall(op) => InstrCodegenOp::Call(op.try_map_children(self)?),
            InstrTyped::LegacyCallDirect(op) => {
                InstrCodegenOp::CallDirect(op.try_map_children(self)?)
            }
            InstrTyped::GetAttrTyped(op) => {
                InstrCodegenOp::GetAttr(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::SetAttrTyped(op) => {
                InstrCodegenOp::SetAttr(op.try_map_children(self)?.into_legacy())
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

#[track_caller]
pub fn try_lower_typed_instr_to_codegen_legacy(instr: InstrTyped) -> Result<InstrCodegen, String> {
    let caller = std::panic::Location::caller();
    TypedToCodegen.try_map_instr(instr).map_err(|err| {
        format!(
            "{err} [typed_to_codegen_legacy caller={}:{}]",
            caller.file(),
            caller.line()
        )
    })
}

#[track_caller]
pub fn try_lower_typed_term_to_codegen_legacy(
    term: BlockTerm<InstrTyped>,
) -> Result<BlockTerm<InstrCodegen>, String> {
    let caller = std::panic::Location::caller();
    TypedToCodegen.try_map_term(term).map_err(|err| {
        format!(
            "{err} [typed_to_codegen_legacy caller={}:{}]",
            caller.file(),
            caller.line()
        )
    })
}

pub fn try_lower_typed_module_to_codegen_legacy(
    module: BlockPyModule<TypedCodegenModuleShape>,
) -> Result<BlockPyModule<CodegenModuleShape>, String> {
    validate_typed_module_call_access_plans(&module)?;
    TypedToCodegen.try_map_module(module)
}

impl InstrWithConstantNone for InstrCodegenOp {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
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
    type Extra = ();
}

impl InstrWithConstantNone for InstrWithAwaitAndYield {
    fn constant_none() -> Self {
        runtime_name_load("NONE")
    }
}

#[derive(Clone, derive_more::From, DelegateMatchDefault)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
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
    type Extra = ();
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
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, Debug)]
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
    type Extra = ();
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
    MakeCell(#[rkyv(omit_bounds)] MakeCell<Self>),
    CellRef(CellRef),
    MakeFunctionWithClosure(#[rkyv(omit_bounds)] MakeFunctionWithClosure<Self>),
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

#[derive(Debug, Clone)]
pub struct CoreModuleShapeWithAwaitAndYield;

impl ModuleShape for CoreModuleShapeWithAwaitAndYield {
    type Instr = InstrWithAwaitAndYield;
    type ModuleConstant = InstrResolved;
}

#[derive(Debug, Clone)]
pub struct CoreModuleShapeWithYield;

impl ModuleShape for CoreModuleShapeWithYield {
    type Instr = InstrWithYield;
    type ModuleConstant = InstrResolved;
}

#[derive(Debug, Clone)]
pub struct CoreModuleShape;

impl ModuleShape for CoreModuleShape {
    type Instr = InstrLow<UnresolvedName>;
    type ModuleConstant = InstrResolved;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ResolvedStorageModuleShape;

impl ModuleShape for ResolvedStorageModuleShape {
    type Instr = InstrResolved;
    type ModuleConstant = InstrResolved;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenModuleShape;

impl ModuleShape for CodegenModuleShape {
    type Instr = InstrCodegen;
    type ModuleConstant = InstrResolved;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CodegenTempLocal {
    pub name: String,
    pub location: LocalLocation,
}

impl CodegenTempLocal {
    pub fn resolved_name(&self) -> ResolvedName {
        ResolvedName {
            id: self.name.clone().into(),
            location: NameLocation::Local(self.location),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CodegenTempAllocationError {
    MissingStorageLayout,
}

pub fn try_allocate_codegen_stack_temp(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    prefix: &str,
) -> Result<CodegenTempLocal, CodegenTempAllocationError> {
    let name = function.name_gen.next_tmp_name(prefix).as_str().to_string();
    let layout = function
        .storage_layout
        .as_mut()
        .ok_or(CodegenTempAllocationError::MissingStorageLayout)?;
    let location = LocalLocation(
        u32::try_from(layout.stack_slots().len())
            .expect("codegen stack slot index should fit in u32"),
    );
    layout.ensure_stack_slot(name.clone());
    Ok(CodegenTempLocal { name, location })
}

pub fn allocate_codegen_stack_temp(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    prefix: &str,
) -> CodegenTempLocal {
    try_allocate_codegen_stack_temp(function, prefix)
        .expect("codegen function should have storage before allocating stack temp")
}

#[derive(Debug, Clone)]
pub struct TypedCodegenModuleShape;

impl ModuleShape for TypedCodegenModuleShape {
    type Instr = InstrTyped;
    type ModuleConstant = InstrResolved;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenUnidentifiedModuleShape;

impl ModuleShape for CodegenUnidentifiedModuleShape {
    type Instr = InstrCodegen;
    type ModuleConstant = InstrResolved;
}

pub(crate) use blockpy_generators::lower_yield_in_lowered_core_blockpy_module_bundle;
pub use blockpy_to_bb::{lower_try_jump_exception_flow, normalize_bb_module_strings};
pub use direct_call_transform::{
    rewrite_profiled_function_call_store_sites,
    rewrite_profiled_function_call_store_sites_with_constructor_targets,
    DirectCallStoreRewriteStats,
};
pub use escape_analysis::{
    straightline_field_initializer_rejection_reason, summarize_module_escapes,
    ConstructorFieldAccess, ConstructorFieldStore, ConstructorFieldValue, EscapeSummaryModule,
    FieldInitializerConstructorSummary, FunctionEscapeSummary,
    NonEscapingConstructorAllocationSummary, NonEscapingConstructorSummary,
};
pub use inline_plan::{
    plan_module_inlining, FunctionInlinePlan, InlinePlanModule, StraightlineConstructorInlinePlan,
};
pub use inline_sites::{
    collect_inline_call_sites, InlineCallSiteModule, StraightlineConstructorCallSite,
};
pub use inline_transform::{
    bind_simple_direct_call_inline_args, bind_simple_direct_method_inline_args,
    build_cross_module_direct_method_inline_fragment_to_target,
    build_direct_method_inline_fragment_to_target, build_single_block_inline_fragment,
    build_single_block_inline_fragment_to_target, build_single_block_inline_fragment_with_bindings,
    inline_and_scalar_replace_until_fixed_point,
    inline_and_scalar_replace_with_callees_until_fixed_point,
    inline_direct_call_stores_with_callees, inline_simple_direct_call_stores,
    rewrite_static_runtime_constructor_call_stores,
    scalar_replace_non_escaping_constructor_allocations, InlineCallee, InlineFragment, InlineLocal,
    InlineRewriteStats, InlineScalarRewriteStats, InlineUnsupportedReason, InlineValueBindings,
    ScalarReplacementStats,
};
pub use instr_id::{
    assign_function_instr_ids, assign_missing_codegen_function_instr_ids, assign_module_instr_ids,
    reassign_codegen_function_instr_ids, reassign_codegen_module_instr_ids,
    validate_codegen_instr_ids,
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
    call_target_counter_instrumentation_enabled, define_bb_module_deopt_entry_counters,
    deopt_entry_counter_instrumentation_enabled, instrument_bb_module_for_trace,
    instrument_bb_module_with_block_entry_counters, instrument_bb_module_with_call_target_counters,
    instrument_bb_module_with_global_load_counters, instrument_bb_module_with_locality_counters,
    instrument_bb_module_with_refcount_counters, locality_counter_instrumentation_enabled,
    parse_trace_env, refcount_counter_instrumentation_enabled,
    specialization_runtime_logging_enabled,
};
pub use value_facts::{
    infer_module_value_facts, BoolFacts, BoolSingletonFact, CallableFact, EnvFacts, FactStore,
    I32Facts, I64Facts, NoneFact, ProvenanceFact, PyExactType, PyObjFacts, RefcountFact,
    RuntimeHelperId, RuntimeHelperSignature, RuntimeSingleton, ThrowSpec, TruthinessFact, TypeFact,
    ValueFacts,
};

pub(crate) use global_index::lower_global_index_in_resolved_module_default;
pub(crate) use name_binding::lower_name_binding_in_core_blockpy_module_with_options;
pub fn relabel_dense_bb_module<P: ModuleShape>(module: &mut BlockPyModule<P>) {
    for callable in &mut module.callable_defs {
        relabel_blockpy_blocks_dense(&mut callable.blocks);
    }
}

#[cfg(test)]
mod typed_codegen_tests {
    use super::*;
    use crate::block_py::{ChildVisitable, Visit, VisitMut};

    #[derive(Default)]
    struct LegacyInstrCounter {
        total: usize,
        truthy: usize,
        loads: usize,
        binops: usize,
        typed_calls: usize,
        guarded_callable_calls: usize,
        guarded_method_calls: usize,
        direct_callable_calls: usize,
        direct_method_calls: usize,
        direct_call_guard_tests: usize,
        typed_getattrs: usize,
        typed_setattrs: usize,
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
            if matches!(expr, InstrTyped::CallTyped(_)) {
                self.typed_calls += 1;
            }
            if matches!(expr, InstrTyped::GuardedCallableCallTyped(_)) {
                self.guarded_callable_calls += 1;
            }
            if matches!(expr, InstrTyped::GuardedMethodCallTyped(_)) {
                self.guarded_method_calls += 1;
            }
            if matches!(expr, InstrTyped::DirectCallableCallTyped(_)) {
                self.direct_callable_calls += 1;
            }
            if matches!(expr, InstrTyped::DirectMethodCallTyped(_)) {
                self.direct_method_calls += 1;
            }
            if matches!(expr, InstrTyped::DirectCallGuardTest(_)) {
                self.direct_call_guard_tests += 1;
            }
            if matches!(expr, InstrTyped::GetAttrTyped(_)) {
                self.typed_getattrs += 1;
            }
            if matches!(expr, InstrTyped::SetAttrTyped(_)) {
                self.typed_setattrs += 1;
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

    #[derive(Default)]
    struct TypedExtraFactCounter {
        extras: usize,
        facts: usize,
        none_singletons: usize,
        bools: usize,
    }

    impl Visit<InstrTyped> for TypedExtraFactCounter {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(extra) = expr.typed_extra() {
                self.extras += 1;
                if let Some(facts) = extra.result_facts() {
                    self.facts += 1;
                    if facts.as_pyobj().is_some_and(PyObjFacts::is_none) {
                        self.none_singletons += 1;
                    }
                    if matches!(facts, ValueFacts::Bool(_)) {
                        self.bools += 1;
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    fn count_typed_extra_facts(
        module: &BlockPyModule<TypedCodegenModuleShape>,
    ) -> TypedExtraFactCounter {
        let mut counter = TypedExtraFactCounter::default();
        for function in &module.callable_defs {
            counter.visit_fn(function);
        }
        counter
    }

    fn typed_function_by_qualname_mut<'a>(
        module: &'a mut BlockPyModule<TypedCodegenModuleShape>,
        qualname: &str,
    ) -> &'a mut BlockPyFunction<TypedCodegenModuleShape> {
        module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing typed function {qualname}"))
    }

    fn codegen_function_id_by_qualname(
        module: &BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> RuntimeFunctionId {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing codegen function {qualname}"))
            .function_id
    }

    fn replace_first_typed_call_access(
        function: &mut BlockPyFunction<TypedCodegenModuleShape>,
        access: TypedCallAccessPlan,
    ) {
        struct Replacer {
            access: Option<TypedCallAccessPlan>,
        }

        impl VisitMut<InstrTyped> for Replacer {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let Some(access) = self.access.take() {
                    if let InstrTyped::CallTyped(call) = expr {
                        call.access = access;
                        return;
                    }
                    self.access = Some(access);
                }
                expr.visit_children_mut(self);
            }
        }

        let mut replacer = Replacer {
            access: Some(access),
        };
        replacer.visit_fn_mut(function);
        assert!(
            replacer.access.is_none(),
            "test function should contain a typed call"
        );
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
        assert_eq!(
            counter.non_legacy,
            counter.loads
                + counter.binops
                + counter.typed_calls
                + counter.guarded_callable_calls
                + counter.guarded_method_calls
                + counter.direct_callable_calls
                + counter.direct_method_calls
                + counter.direct_call_guard_tests
                + counter.typed_getattrs
                + counter.typed_setattrs
        );
    }

    #[test]
    fn typed_instr_extras_start_without_result_facts() {
        let lowered = crate::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
            .expect("source should lower");

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let counter = count_typed_extra_facts(&typed);

        assert!(counter.extras > 0);
        assert_eq!(counter.facts, 0);
    }

    #[test]
    fn annotate_typed_module_value_facts_materializes_result_facts() {
        let lowered = crate::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
            .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.codegen_module);
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let changed = annotate_typed_module_value_facts(&mut typed, &facts);
        let counter = count_typed_extra_facts(&typed);

        assert!(changed > 0);
        assert!(counter.facts > 0);
        assert!(counter.none_singletons > 0);
    }

    #[test]
    fn validate_typed_function_value_facts_requires_embedded_result_facts() {
        let lowered = crate::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
            .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.codegen_module);
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let function = typed_function_by_qualname_mut(&mut typed, "f");
        assert!(
            validate_typed_function_value_facts(function).is_err(),
            "typed facts validation should reject an unannotated typed function"
        );

        annotate_typed_function_value_facts(function, &facts);
        validate_typed_function_value_facts(function)
            .expect("annotated typed function should validate");
    }

    #[test]
    fn lower_typed_if_tests_to_truthy_embeds_bool_result_facts() {
        let lowered = crate::lower_python_to_blockpy_for_testing(
            "def f(value):\n    if value:\n        return None\n    return None\n",
        )
        .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.codegen_module);
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let function = typed_function_by_qualname_mut(&mut typed, "f");

        annotate_typed_function_value_facts(function, &facts);
        let function = lower_typed_function_if_tests_to_truthy(function.clone());
        validate_typed_function_value_facts(&function)
            .expect("truthy-lowered typed function should carry result facts");

        let mut counter = TypedExtraFactCounter::default();
        counter.visit_fn(&function);
        assert!(counter.bools > 0);
    }

    #[test]
    fn refresh_typed_function_value_facts_recovers_binop_result_facts() {
        struct FirstBinOpFactClearer {
            cleared: bool,
        }

        impl VisitMut<InstrTyped> for FirstBinOpFactClearer {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if !self.cleared {
                    if let InstrTyped::BinOp(op) = expr {
                        self.cleared = op.extra_mut().clear_result_facts();
                        return;
                    }
                }
                expr.visit_children_mut(self);
            }
        }

        let lowered = crate::lower_python_to_blockpy_for_testing("def f():\n    return 1 + 2\n")
            .expect("source should lower");
        let facts = infer_module_value_facts(&lowered.codegen_module);
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let function = typed_function_by_qualname_mut(&mut typed, "f");

        annotate_typed_function_value_facts(function, &facts);
        let mut clearer = FirstBinOpFactClearer { cleared: false };
        clearer.visit_fn_mut(function);
        assert!(
            clearer.cleared,
            "test function should contain an annotated typed binop"
        );
        assert!(
            validate_typed_function_value_facts(function).is_err(),
            "clearing binop facts should break typed fact validation"
        );

        assert!(refresh_typed_function_value_facts(function) > 0);
        validate_typed_function_value_facts(function)
            .expect("refreshed typed function should validate");
    }

    #[test]
    fn lower_codegen_module_to_typed_makes_attrs_first_class() {
        let lowered = crate::lower_python_to_blockpy_for_testing(
            "def f(obj, value):\n    obj.x = value\n    return obj.x\n",
        )
        .expect("source should lower");

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let mut counter = LegacyInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                for instr in &block.body {
                    counter.visit_instr(instr);
                }
                counter.visit_term(&block.term);
            }
        }

        assert!(counter.typed_getattrs > 0);
        assert!(counter.typed_setattrs > 0);
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
            counter.truthy
                + counter.loads
                + counter.binops
                + counter.typed_calls
                + counter.guarded_callable_calls
                + counter.guarded_method_calls
                + counter.direct_callable_calls
                + counter.direct_method_calls
                + counter.direct_call_guard_tests
                + counter.typed_getattrs
                + counter.typed_setattrs
        );
        assert!(
            try_lower_typed_module_to_codegen_legacy(typed).is_err(),
            "typed truthiness should not silently lower through the legacy adapter"
        );
    }

    #[test]
    fn validates_guarded_method_typed_call_access_plan_shape() {
        let lowered = crate::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return it.__next__()\n",
        )
        .expect("source should lower");
        let next_id =
            codegen_function_id_by_qualname(&lowered.codegen_module, "IterRange.__next__");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "__next__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: next_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        validate_typed_function_call_access_plans(caller).expect("guarded method shape is valid");
    }

    #[test]
    fn lowers_guarded_callable_typed_call_access_plan_to_instr() {
        let lowered = crate::lower_python_to_blockpy_for_testing(
            "def add(a, b):\n    return a + b\n\n\
def caller(a, b):\n    return add(a, b)\n",
        )
        .expect("source should lower");
        let add_id = codegen_function_id_by_qualname(&lowered.codegen_module, "add");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedCallable {
                function_guards: vec![TypedDirectFunctionCallGuard {
                    function_id: add_id,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![
                            TypedDirectCallArgSource::Provided(0),
                            TypedDirectCallArgSource::Provided(1),
                        ],
                    },
                }],
                constructor_guards: Vec::new(),
            },
        );

        assert_eq!(lower_typed_function_call_access_plan_instrs(caller), 1);
        validate_typed_function_call_access_plans(caller)
            .expect("lowered guarded callable shape is valid");

        let mut counter = LegacyInstrCounter::default();
        for block in &caller.blocks {
            for instr in &block.body {
                counter.visit_instr(instr);
            }
            counter.visit_term(&block.term);
        }
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_callable_calls, 1);
    }

    #[test]
    fn lowers_guarded_method_typed_call_access_plan_to_instr() {
        let lowered = crate::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return it.__next__()\n",
        )
        .expect("source should lower");
        let next_id =
            codegen_function_id_by_qualname(&lowered.codegen_module, "IterRange.__next__");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "__next__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: next_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        assert_eq!(lower_typed_function_call_access_plan_instrs(caller), 1);
        validate_typed_function_call_access_plans(caller)
            .expect("lowered guarded method shape is valid");

        let mut counter = LegacyInstrCounter::default();
        for block in &caller.blocks {
            for instr in &block.body {
                counter.visit_instr(instr);
            }
            counter.visit_term(&block.term);
        }
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_method_calls, 1);
    }

    #[test]
    fn rejects_guarded_method_typed_call_access_without_getattr_target() {
        let lowered =
            crate::lower_python_to_blockpy_for_testing("def caller(fn):\n    return fn()\n")
                .expect("source should lower");
        let caller_id = codegen_function_id_by_qualname(&lowered.codegen_module, "caller");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        replace_first_typed_call_access(
            caller,
            TypedCallAccessPlan::GuardedMethod {
                method_name: "__call__".to_string(),
                method_guards: vec![TypedDirectMethodCallGuard {
                    function_id: caller_id,
                    owner_type_ref: TypedAttrOwnerRef::TypeKey {
                        module_name: "__main__".to_string(),
                        qualname: "Caller".to_string(),
                    },
                    type_version: 1,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![TypedDirectCallArgSource::Provided(0)],
                    },
                }],
            },
        );

        let err = validate_typed_function_call_access_plans(caller)
            .expect_err("guarded method without GetAttr should be rejected");
        assert!(err.contains("requires a GetAttr call target"));
    }

    #[test]
    fn direct_call_guard_test_is_first_class_typed_instr() {
        let load: InstrTyped = runtime_name_load("NONE");
        let guard = InstrTyped::DirectCallGuardTest(TypedDirectCallGuardTest::new(
            load,
            TypedDirectCallGuardTestKind::RuntimeFunctionId {
                function_id: RuntimeFunctionId::from_raw_parts(0, 7),
            },
        ));

        let mut counter = LegacyInstrCounter::default();
        counter.visit_instr(&guard);

        assert_eq!(counter.direct_call_guard_tests, 1);
        assert_eq!(
            counter.non_legacy,
            counter.loads + counter.direct_call_guard_tests
        );
        assert!(
            matches!(
                try_lower_typed_instr_to_codegen_legacy(guard),
                Ok(InstrCodegen::DirectFunctionIdGuardTest(_))
            ),
            "function-id direct-call guards should lower to the codegen guard representation"
        );

        let receiver_guard = InstrTyped::DirectCallGuardTest(TypedDirectCallGuardTest::new(
            runtime_name_load::<InstrTyped>("NONE"),
            TypedDirectCallGuardTestKind::ExactReceiverTypeVersion {
                owner_type_ref: TypedAttrOwnerRef::TypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "Receiver".to_string(),
                },
                type_version: 1,
            },
        ));
        assert!(
            matches!(
                try_lower_typed_instr_to_codegen_legacy(receiver_guard),
                Ok(InstrCodegen::DirectReceiverTypeVersionGuardTest(_))
            ),
            "receiver type-version direct-call guards should lower to the codegen guard representation"
        );

        let non_function_guard = InstrTyped::DirectCallGuardTest(TypedDirectCallGuardTest::new(
            runtime_name_load::<InstrTyped>("NONE"),
            TypedDirectCallGuardTestKind::ExactCallableTypeVersion {
                owner_type_ref: TypedAttrOwnerRef::TypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "Callable".to_string(),
                },
                type_version: 1,
            },
        ));
        assert!(
            try_lower_typed_instr_to_codegen_legacy(non_function_guard).is_err(),
            "callable type-version direct-call guard tests still require typed codegen emission"
        );
    }

    #[test]
    fn direct_callable_call_is_first_class_typed_instr() {
        let func: InstrTyped = runtime_name_load("NONE");
        let arg: InstrTyped = runtime_name_load("NONE");
        let direct_call = InstrTyped::DirectCallableCallTyped(TypedDirectCallableCall::new(
            func,
            vec![CallArgPositional::Positional(arg)],
            TypedDirectCallableCallGuard::Function(TypedDirectFunctionCallGuard {
                function_id: RuntimeFunctionId::from_raw_parts(0, 8),
                arg_plan: TypedDirectCallArgPlan {
                    sources: vec![TypedDirectCallArgSource::Provided(0)],
                },
            }),
        ));

        let mut counter = LegacyInstrCounter::default();
        counter.visit_instr(&direct_call);

        assert_eq!(counter.direct_callable_calls, 1);
        assert_eq!(counter.loads, 2);
        assert_eq!(
            counter.non_legacy,
            counter.loads + counter.direct_callable_calls
        );
        assert!(
            try_lower_typed_instr_to_codegen_legacy(direct_call).is_err(),
            "typed direct callable calls should not silently lower through the legacy adapter"
        );
    }

    #[test]
    fn direct_method_call_is_first_class_typed_instr() {
        let receiver: InstrTyped = runtime_name_load("NONE");
        let arg: InstrTyped = runtime_name_load("NONE");
        let direct_call = InstrTyped::DirectMethodCallTyped(TypedDirectMethodCall::new(
            receiver,
            vec![CallArgPositional::Positional(arg)],
            "__next__",
            TypedDirectMethodCallGuard {
                function_id: RuntimeFunctionId::from_raw_parts(0, 9),
                owner_type_ref: TypedAttrOwnerRef::TypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "IterRange".to_string(),
                },
                type_version: 1,
                arg_plan: TypedDirectCallArgPlan {
                    sources: vec![
                        TypedDirectCallArgSource::Provided(0),
                        TypedDirectCallArgSource::Provided(1),
                    ],
                },
            },
        ));

        let mut counter = LegacyInstrCounter::default();
        counter.visit_instr(&direct_call);

        assert_eq!(counter.direct_method_calls, 1);
        assert_eq!(counter.loads, 2);
        assert_eq!(
            counter.non_legacy,
            counter.loads + counter.direct_method_calls
        );
        assert!(
            try_lower_typed_instr_to_codegen_legacy(direct_call).is_err(),
            "typed direct method calls should not silently lower through the legacy adapter"
        );
    }
}

#[cfg(test)]
mod test;
