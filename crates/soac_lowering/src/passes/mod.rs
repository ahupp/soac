pub(crate) mod ast_symbol_analysis;
pub(crate) mod ast_to_ast;
pub(crate) mod ast_to_instr;
pub(crate) mod blockpy_expr_simplify;
mod blockpy_generators;
mod blockpy_to_bb;
pub(crate) mod core_await_lower;
mod global_index;
mod instr_id;
mod instrument;
mod name_binding;
pub(crate) mod ruff_to_blockpy;
mod trace;

use crate::block_py::{
    cfg::relabel_blockpy_blocks_dense, define_instr, define_ruff_instr, runtime_name_load, Await,
    BinOp, BlockPyModule, Call, CallArgKeyword, CallArgPositional, CallDirect, CalleeFunctionId,
    CellRef, CellRefForName, ChildVisitable, Del, DelItem, ExprAttribute, ExprBoolOp,
    ExprBooleanLiteral, ExprBytesLiteral, ExprCompare, ExprDict, ExprDictComp, ExprEllipsisLiteral,
    ExprFString, ExprGenerator, ExprIf, ExprIpyEscapeCommand, ExprLambda, ExprList, ExprListComp,
    ExprName, ExprNamed, ExprNoneLiteral, ExprNumberLiteral, ExprSet, ExprSetComp, ExprSlice,
    ExprStarred, ExprStringLiteral, ExprSubscript, ExprTString, ExprTuple, GetAttr, GetItem,
    HasMeta, IncrementCounter, Instr, InstrId, InstrWithConstantNone, LiteralValue, Load, MakeCell,
    MakeFunction, MakeFunctionWithClosure, MapInstr, Mappable, Meta, ModuleShape, NameLike,
    PrettyPrint, PrettyPrinter, ResolvedName, RuntimeFunctionId, RuntimeName, SetAttr, SetItem,
    StmtAnnAssign, StmtAssert, StmtAssign, StmtAugAssign, StmtBreak, StmtClassDef, StmtContinue,
    StmtDelete, StmtExpr, StmtFor, StmtFunctionDef, StmtGlobal, StmtIf, StmtImport, StmtImportFrom,
    StmtIpyEscapeCommand, StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn, StmtTry,
    StmtTypeAlias, StmtWhile, StmtWith, Store, TryMapInstr, Tuple, UnaryOp, UnresolvedName,
    WithMeta, Yield, YieldFrom,
};
use ruff_python_ast::{self as ast};
use soac_macros::{enum_broadcast, DelegateMatchDefault};
use std::collections::{HashMap, HashSet};

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
    DirectCallableTypeVersionGuardTest(
        #[rkyv(omit_bounds)] DirectCallableTypeVersionGuardTest<Self>,
    ),
    DirectReceiverTypeVersionGuardTest(
        #[rkyv(omit_bounds)] DirectReceiverTypeVersionGuardTest<Self>,
    ),
    Tuple(#[rkyv(omit_bounds)] Tuple<Self>),
    Call(#[rkyv(omit_bounds)] Call<Self>),
    CallDirect(#[rkyv(omit_bounds)] CallDirect<Self>),
    DirectCallableCall(#[rkyv(omit_bounds)] TypedDirectCallableCall<Self>),
    DirectMethodCall(#[rkyv(omit_bounds)] TypedDirectMethodCall<Self>),
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
    pub struct DirectCallableTypeVersionGuardTest<E> {
        value: Box<E>,
        owner_type_ref: TypedAttrOwnerRef,
        type_version: u32,
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

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum TypedDirectCallArgSource {
    Provided(usize),
    DefaultSentinel,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypedDirectCallArgPlan {
    pub sources: Vec<TypedDirectCallArgSource>,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypedDirectFunctionCallGuard {
    pub function_id: RuntimeFunctionId,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypedDirectMethodCallGuard {
    pub function_id: RuntimeFunctionId,
    pub owner_type_ref: TypedAttrOwnerRef,
    pub type_version: u32,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypedDirectConstructorCallGuard {
    pub function_id: RuntimeFunctionId,
    pub owner_type_ref: TypedAttrOwnerRef,
    pub type_version: u32,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypedCallEmissionPlans {
    pub by_source: HashMap<InstrId, TypedCallEmissionPlan>,
}

impl TypedCallEmissionPlans {
    pub fn is_empty(&self) -> bool {
        self.by_source.is_empty()
    }

    pub fn sources(&self) -> HashSet<InstrId> {
        self.by_source.keys().copied().collect()
    }

    pub fn target_function_ids(&self) -> Vec<RuntimeFunctionId> {
        self.by_source
            .values()
            .flat_map(TypedCallEmissionPlan::target_function_ids)
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedCallEmissionPlan {
    Callable {
        function_guards: Vec<TypedDirectFunctionCallGuard>,
        constructor_guards: Vec<TypedDirectConstructorCallGuard>,
    },
    Method {
        method_name: String,
        method_guards: Vec<TypedDirectMethodCallGuard>,
    },
}

impl TypedCallEmissionPlan {
    pub fn target_function_ids(&self) -> Vec<RuntimeFunctionId> {
        match self {
            Self::Callable {
                function_guards,
                constructor_guards,
            } => function_guards
                .iter()
                .map(|guard| guard.function_id)
                .chain(constructor_guards.iter().map(|guard| guard.function_id))
                .collect(),
            Self::Method { method_guards, .. } => method_guards
                .iter()
                .map(|guard| guard.function_id)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Callable {
                function_guards,
                constructor_guards,
            } => function_guards.is_empty() && constructor_guards.is_empty(),
            Self::Method { method_guards, .. } => method_guards.is_empty(),
        }
    }
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

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CodegenUnidentifiedModuleShape;

impl ModuleShape for CodegenUnidentifiedModuleShape {
    type Instr = InstrCodegen;
    type ModuleConstant = InstrResolved;
}

pub(crate) use blockpy_generators::lower_yield_in_lowered_core_blockpy_module_bundle;
pub use blockpy_to_bb::{lower_try_jump_exception_flow, normalize_bb_module_strings};
pub use instr_id::{
    assign_function_instr_ids, assign_missing_codegen_function_instr_ids, assign_module_instr_ids,
    reassign_codegen_function_instr_ids, reassign_codegen_module_instr_ids,
    validate_codegen_instr_ids,
};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
pub use trace::{
    call_target_counter_instrumentation_enabled, deopt_entry_counter_instrumentation_enabled,
    instrument_bb_module_for_trace, instrument_bb_module_with_block_entry_counters,
    instrument_bb_module_with_call_target_counters, instrument_bb_module_with_global_load_counters,
    instrument_bb_module_with_locality_counters, instrument_bb_module_with_refcount_counters,
    locality_counter_instrumentation_enabled, parse_trace_env,
    refcount_counter_instrumentation_enabled, specialization_runtime_logging_enabled,
};

pub(crate) use global_index::lower_global_index_in_resolved_module_default;
pub(crate) use name_binding::lower_name_binding_in_core_blockpy_module_with_options;
pub fn relabel_dense_bb_module<P: ModuleShape>(module: &mut BlockPyModule<P>) {
    for callable in &mut module.callable_defs {
        relabel_blockpy_blocks_dense(&mut callable.blocks);
    }
}

#[cfg(test)]
mod test;
