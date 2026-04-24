use crate::emit_v3::MechanicalRegionEmission;
use crate::passes::{BoolFacts, FactStore, PyObjFacts, ValueFacts, value_facts};
use crate::plan_v3::{
    ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemGuardKind, ExactListItemShape,
    IndexedGlobalAccessKind, IndexedGlobalFallbackKind, IndexedGlobalGuardKind, RegionPlan,
    ScalarLocalThreadPlan,
};
#[allow(unused_imports)]
use soac_core::block_py;
use soac_core::block_py::IncrementCounter;
#[allow(unused_imports)]
use soac_core::block_py::{
    BinOp, Block, BlockEdge, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, Call,
    CallArgKeyword, CallArgPositional, CallDirect, CalleeFunctionId, CellRef, ChildVisitable, Del,
    DelItem, GetAttr, GetItem, HasMeta, HasSemanticInstrId, Instr, InstrId, InstrKey,
    InstrWithConstantNone, Load, LocalLocation, MakeCell, MakeFunctionWithClosure, MapFunction,
    MapInstr, MapModule, Mappable, Meta, ModuleShape, NameLike, NameLocation, ParamKind,
    PrettyPrint, PrettyPrinter, ResolvedName, RuntimeFunctionId, RuntimeName, SetAttr, SetItem,
    Store, TermIf, TryMapInstr, TryMapModule, TryMapTerm, Tuple, UnaryOp, Visit, VisitMut,
    WithMeta, define_instr, define_ruff_instr,
};
#[allow(unused_imports)]
use soac_lowering::passes::{
    CodegenModuleShape, DirectFunctionIdGuardTest, InstrCodegen, InstrResolved,
};
use soac_macros::{DelegateMatchDefault, enum_broadcast};
use std::collections::{HashMap, HashSet};

fn runtime_name_load<E>(name: &str) -> E
where
    E: Instr + From<Load<E>>,
{
    Load::new(E::Name::runtime_name(name)).into()
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
        V: block_py::Visit<E> + ?Sized,
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
        V: block_py::VisitMut<E> + ?Sized,
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
        V: block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.func);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: block_py::VisitMut<E> + ?Sized,
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
        V: block_py::Visit<E> + ?Sized,
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
        V: block_py::VisitMut<E> + ?Sized,
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
        V: block_py::Visit<E> + ?Sized,
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
        V: block_py::VisitMut<E> + ?Sized,
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
        V: block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(&self.receiver);
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: block_py::VisitMut<E> + ?Sized,
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
    RuntimeFunctionId { function_id: RuntimeFunctionId },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedIndexedGlobalPlanSource {
    OptimizationPlanV3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedIndexedGlobalAccessPlan {
    pub source: TypedIndexedGlobalPlanSource,
    pub instr_id: InstrId,
    pub access: IndexedGlobalAccessKind,
    pub module_name: String,
    pub name: String,
    pub expected_index: u32,
    pub guard: IndexedGlobalGuardKind,
    pub fallback: IndexedGlobalFallbackKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedExactListItemPlanSource {
    OptimizationPlanV3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExactListItemAccessPlan {
    pub source: TypedExactListItemPlanSource,
    pub instr_id: InstrId,
    pub access: ExactListItemAccessKind,
    pub shape: ExactListItemShape,
    pub guard: ExactListItemGuardKind,
    pub fallback: ExactListItemFallbackKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedExactIntPlanSource {
    OptimizationPlanV3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExactIntBranchPlan {
    pub source: TypedExactIntPlanSource,
    pub instr_id: InstrId,
    pub hot_plan: RegionPlan,
    pub hot_region: MechanicalRegionEmission,
    pub fallback_plan: RegionPlan,
    pub fallback_region: MechanicalRegionEmission,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExactIntReturnPlan {
    pub source: TypedExactIntPlanSource,
    pub instr_id: InstrId,
    pub hot_plan: RegionPlan,
    pub hot_region: MechanicalRegionEmission,
    pub fallback_plan: RegionPlan,
    pub fallback_region: MechanicalRegionEmission,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExactIntScalarThreadPlan {
    pub source: TypedExactIntPlanSource,
    pub store_instr_id: InstrId,
    pub producer_instr_id: InstrId,
    pub consumer_instr_id: InstrId,
    pub thread: ScalarLocalThreadPlan,
    pub producer_hot_plan: RegionPlan,
    pub producer_hot_region: MechanicalRegionEmission,
    pub producer_fallback_plan: RegionPlan,
    pub producer_fallback_region: MechanicalRegionEmission,
    pub consumer_hot_plan: RegionPlan,
    pub consumer_hot_region: MechanicalRegionEmission,
    pub consumer_fallback_plan: RegionPlan,
    pub consumer_fallback_region: MechanicalRegionEmission,
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
    Tuple(Tuple<Self>),
    UnaryOp(UnaryOp<Self>),
    CalleeFunctionId(CalleeFunctionId<Self>),
    CallTyped(TypedCall<Self>),
    GuardedCallableCallTyped(TypedGuardedCallableCall<Self>),
    GuardedMethodCallTyped(TypedGuardedMethodCall<Self>),
    DirectCallableCallTyped(TypedDirectCallableCall<Self>),
    DirectMethodCallTyped(TypedDirectMethodCall<Self>),
    DirectCallGuardTest(TypedDirectCallGuardTest<Self>),
    CallDirect(CallDirect<Self>),
    GetAttrTyped(TypedGetAttr<Self>),
    SetAttrTyped(TypedSetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    MakeCell(MakeCell<Self>),
    IncrementCounter(IncrementCounter),
    CellRef(CellRef),
    MakeFunctionWithClosure(MakeFunctionWithClosure<Self>),
}

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
    pub indexed_global_access: Option<TypedIndexedGlobalAccessPlan>,
    pub exact_list_item_access: Option<TypedExactListItemAccessPlan>,
    pub exact_int_branch: Option<TypedExactIntBranchPlan>,
    pub exact_int_return: Option<TypedExactIntReturnPlan>,
    pub exact_int_scalar_thread: Option<TypedExactIntScalarThreadPlan>,
    pub guard_miss_deopt: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TypedBlockLayoutHint {
    #[default]
    Normal,
    Cold,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypedBlockExtra {
    pub layout: TypedBlockLayoutHint,
}

pub type TypedBlock = Block<InstrTyped, TypedBlockExtra>;

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

    pub fn indexed_global_access_plan(&self) -> Option<&TypedIndexedGlobalAccessPlan> {
        self.indexed_global_access.as_ref()
    }

    pub fn set_indexed_global_access_plan(&mut self, plan: TypedIndexedGlobalAccessPlan) -> bool {
        if self.indexed_global_access.as_ref() == Some(&plan) {
            return false;
        }
        self.indexed_global_access = Some(plan);
        true
    }

    pub fn clear_indexed_global_access_plan(&mut self) -> bool {
        self.indexed_global_access.take().is_some()
    }

    pub fn exact_list_item_access_plan(&self) -> Option<&TypedExactListItemAccessPlan> {
        self.exact_list_item_access.as_ref()
    }

    pub fn set_exact_list_item_access_plan(&mut self, plan: TypedExactListItemAccessPlan) -> bool {
        if self.exact_list_item_access.as_ref() == Some(&plan) {
            return false;
        }
        self.exact_list_item_access = Some(plan);
        true
    }

    pub fn clear_exact_list_item_access_plan(&mut self) -> bool {
        self.exact_list_item_access.take().is_some()
    }

    pub fn exact_int_branch_plan(&self) -> Option<&TypedExactIntBranchPlan> {
        self.exact_int_branch.as_ref()
    }

    pub fn set_exact_int_branch_plan(&mut self, plan: TypedExactIntBranchPlan) -> bool {
        if self.exact_int_branch.as_ref() == Some(&plan) {
            return false;
        }
        self.exact_int_branch = Some(plan);
        true
    }

    pub fn clear_exact_int_branch_plan(&mut self) -> bool {
        self.exact_int_branch.take().is_some()
    }

    pub fn exact_int_return_plan(&self) -> Option<&TypedExactIntReturnPlan> {
        self.exact_int_return.as_ref()
    }

    pub fn set_exact_int_return_plan(&mut self, plan: TypedExactIntReturnPlan) -> bool {
        if self.exact_int_return.as_ref() == Some(&plan) {
            return false;
        }
        self.exact_int_return = Some(plan);
        true
    }

    pub fn clear_exact_int_return_plan(&mut self) -> bool {
        self.exact_int_return.take().is_some()
    }

    pub fn exact_int_scalar_thread_plan(&self) -> Option<&TypedExactIntScalarThreadPlan> {
        self.exact_int_scalar_thread.as_ref()
    }

    pub fn set_exact_int_scalar_thread_plan(
        &mut self,
        plan: TypedExactIntScalarThreadPlan,
    ) -> bool {
        if self.exact_int_scalar_thread.as_ref() == Some(&plan) {
            return false;
        }
        self.exact_int_scalar_thread = Some(plan);
        true
    }

    pub fn clear_exact_int_scalar_thread_plan(&mut self) -> bool {
        self.exact_int_scalar_thread.take().is_some()
    }

    pub fn guard_miss_deopt_enabled(&self) -> bool {
        self.guard_miss_deopt
    }

    pub fn set_guard_miss_deopt_enabled(&mut self, enabled: bool) -> bool {
        if self.guard_miss_deopt == enabled {
            return false;
        }
        self.guard_miss_deopt = enabled;
        true
    }
}

impl InstrTyped {
    pub fn typed_extra(&self) -> Option<&TypedInstrExtra> {
        match self {
            Self::Truthy(op) => Some(&op.extra),
            Self::Load(op) => Some(op.extra()),
            Self::BinOp(op) => Some(op.extra()),
            Self::Tuple(op) => Some(op.extra()),
            Self::UnaryOp(op) => Some(op.extra()),
            Self::CalleeFunctionId(op) => Some(op.extra()),
            Self::CallTyped(op) => Some(&op.extra),
            Self::GuardedCallableCallTyped(op) => Some(&op.extra),
            Self::GuardedMethodCallTyped(op) => Some(&op.extra),
            Self::DirectCallableCallTyped(op) => Some(&op.extra),
            Self::DirectMethodCallTyped(op) => Some(&op.extra),
            Self::DirectCallGuardTest(op) => Some(&op.extra),
            Self::CallDirect(op) => Some(op.extra()),
            Self::GetAttrTyped(op) => Some(&op.extra),
            Self::SetAttrTyped(op) => Some(&op.extra),
            Self::GetItem(op) => Some(op.extra()),
            Self::SetItem(op) => Some(op.extra()),
            Self::DelItem(op) => Some(op.extra()),
            Self::Store(op) => Some(op.extra()),
            Self::Del(op) => Some(op.extra()),
            Self::MakeCell(op) => Some(op.extra()),
            Self::MakeFunctionWithClosure(op) => Some(op.extra()),
            Self::IncrementCounter(_) | Self::CellRef(_) => None,
        }
    }

    pub fn typed_extra_mut(&mut self) -> Option<&mut TypedInstrExtra> {
        match self {
            Self::Truthy(op) => Some(&mut op.extra),
            Self::Load(op) => Some(op.extra_mut()),
            Self::BinOp(op) => Some(op.extra_mut()),
            Self::Tuple(op) => Some(op.extra_mut()),
            Self::UnaryOp(op) => Some(op.extra_mut()),
            Self::CalleeFunctionId(op) => Some(op.extra_mut()),
            Self::CallTyped(op) => Some(&mut op.extra),
            Self::GuardedCallableCallTyped(op) => Some(&mut op.extra),
            Self::GuardedMethodCallTyped(op) => Some(&mut op.extra),
            Self::DirectCallableCallTyped(op) => Some(&mut op.extra),
            Self::DirectMethodCallTyped(op) => Some(&mut op.extra),
            Self::DirectCallGuardTest(op) => Some(&mut op.extra),
            Self::CallDirect(op) => Some(op.extra_mut()),
            Self::GetAttrTyped(op) => Some(&mut op.extra),
            Self::SetAttrTyped(op) => Some(&mut op.extra),
            Self::GetItem(op) => Some(op.extra_mut()),
            Self::SetItem(op) => Some(op.extra_mut()),
            Self::DelItem(op) => Some(op.extra_mut()),
            Self::Store(op) => Some(op.extra_mut()),
            Self::Del(op) => Some(op.extra_mut()),
            Self::MakeCell(op) => Some(op.extra_mut()),
            Self::MakeFunctionWithClosure(op) => Some(op.extra_mut()),
            Self::IncrementCounter(_) | Self::CellRef(_) => None,
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

    pub fn guard_miss_deopt_enabled(&self) -> bool {
        self.typed_extra()
            .is_some_and(TypedInstrExtra::guard_miss_deopt_enabled)
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
            InstrCodegen::BinOp(op) => InstrTyped::BinOp(op.map_children(self)),
            InstrCodegen::Tuple(op) => InstrTyped::Tuple(op.map_children(self)),
            InstrCodegen::UnaryOp(op) => InstrTyped::UnaryOp(op.map_children(self)),
            InstrCodegen::CalleeFunctionId(op) => {
                InstrTyped::CalleeFunctionId(op.map_children(self))
            }
            InstrCodegen::DirectFunctionIdGuardTest(op) => {
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
            InstrCodegen::Call(op) => {
                InstrTyped::CallTyped(TypedCall::from_legacy(op.map_children(self)))
            }
            InstrCodegen::CallDirect(op) => InstrTyped::CallDirect(op.map_children(self)),
            InstrCodegen::GetAttr(op) => {
                InstrTyped::GetAttrTyped(TypedGetAttr::from_legacy(op.map_children(self)))
            }
            InstrCodegen::SetAttr(op) => {
                InstrTyped::SetAttrTyped(TypedSetAttr::from_legacy(op.map_children(self)))
            }
            InstrCodegen::GetItem(op) => InstrTyped::GetItem(op.map_children(self)),
            InstrCodegen::SetItem(op) => InstrTyped::SetItem(op.map_children(self)),
            InstrCodegen::DelItem(op) => InstrTyped::DelItem(op.map_children(self)),
            InstrCodegen::Load(op) => InstrTyped::Load(op.map_children(self)),
            InstrCodegen::Store(op) => InstrTyped::Store(op.map_children(self)),
            InstrCodegen::Del(op) => InstrTyped::Del(op.map_children(self)),
            InstrCodegen::MakeCell(op) => InstrTyped::MakeCell(op.map_children(self)),
            InstrCodegen::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrCodegen::CellRef(op) => InstrTyped::CellRef(op),
            InstrCodegen::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.map_children(self))
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

struct MissingTypedBlockInstrIdAssigner<'a> {
    next_instr_index: u32,
    used: &'a mut HashSet<InstrId>,
}

impl MissingTypedBlockInstrIdAssigner<'_> {
    fn assign(&mut self, expr: &mut InstrTyped) {
        if expr.try_semantic_instr_id().is_some() {
            return;
        }
        while self.used.contains(&InstrId::new(self.next_instr_index)) {
            self.next_instr_index = self
                .next_instr_index
                .checked_add(1)
                .expect("per-function instruction count should fit in u32");
        }
        let mut meta = expr.meta();
        let instr_id = InstrId::new(self.next_instr_index);
        meta.instr_id = Some(instr_id);
        self.used.insert(instr_id);
        self.next_instr_index = self
            .next_instr_index
            .checked_add(1)
            .expect("per-function instruction count should fit in u32");
        *expr = expr.clone().with_meta(meta);
    }
}

impl VisitMut<InstrTyped> for MissingTypedBlockInstrIdAssigner<'_> {
    fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
        self.assign(expr);
        expr.visit_children_mut(self);
    }
}

pub fn assign_missing_typed_function_instr_ids(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
) {
    let mut next_instr_index = 0;
    let mut used = HashSet::new();
    {
        struct MaxIdCollector<'a> {
            next_instr_index: &'a mut u32,
            used: &'a mut HashSet<InstrId>,
        }

        impl Visit<InstrTyped> for MaxIdCollector<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let Some(instr_id) = expr.try_semantic_instr_id() {
                    self.used.insert(instr_id);
                    *self.next_instr_index = (*self.next_instr_index).max(
                        instr_id
                            .index()
                            .checked_add(1)
                            .expect("per-function instruction count should fit in u32"),
                    );
                }
                expr.visit_children(self);
            }
        }

        let mut collector = MaxIdCollector {
            next_instr_index: &mut next_instr_index,
            used: &mut used,
        };
        collector.visit_fn(function);
    }

    let mut assigner = MissingTypedBlockInstrIdAssigner {
        next_instr_index,
        used: &mut used,
    };
    for block in &mut function.blocks {
        assigner.visit_block_mut(block);
    }
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
        InstrTyped::UnaryOp(op) => annotate_pyobject_borrowed_input_demand(op.operand.as_mut()),
        InstrTyped::Tuple(op) => op
            .values
            .iter_mut()
            .map(annotate_pyobject_borrowed_input_demand)
            .sum(),
        InstrTyped::CalleeFunctionId(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
        }
        InstrTyped::Store(store) => {
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
        InstrTyped::CallDirect(call) => {
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
        InstrTyped::GetItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
        }
        InstrTyped::SetItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.replacement.as_mut())
        }
        InstrTyped::DelItem(op) => {
            annotate_pyobject_borrowed_input_demand(op.value.as_mut())
                + annotate_pyobject_borrowed_input_demand(op.index.as_mut())
        }
        InstrTyped::MakeFunctionWithClosure(op) => {
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
        InstrTyped::Load(op) => op
            .extra()
            .result_facts()
            .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::BinOp(op) => value_facts::infer_binop_result_facts(
            op.kind,
            op.left.result_facts()?,
            op.right.result_facts()?,
        )
        .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::UnaryOp(op) => {
            value_facts::infer_unary_result_facts(op.kind, op.operand.result_facts()?)
                .or(Some(ValueFacts::unknown_pyobj()))
        }
        InstrTyped::Tuple(_) => Some(ValueFacts::PyObj(PyObjFacts::known_not_none())),
        InstrTyped::CallTyped(op) => infer_typed_call_result_facts(
            op.func.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        )
        .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::CallDirect(op) => infer_typed_call_result_facts(
            op.callable.as_ref(),
            op.args.as_slice(),
            op.keywords.as_slice(),
        )
        .or(Some(ValueFacts::unknown_pyobj())),
        InstrTyped::DirectCallGuardTest(_) => Some(ValueFacts::Bool(BoolFacts)),
        InstrTyped::SetAttrTyped(_)
        | InstrTyped::Store(_)
        | InstrTyped::SetItem(_)
        | InstrTyped::DelItem(_)
        | InstrTyped::Del(_) => Some(ValueFacts::PyObj(PyObjFacts::none_singleton())),
        _ => expr.typed_extra().map(|_| ValueFacts::unknown_pyobj()),
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
        if let BlockTerm::IfTerm(if_term) = &mut block.term {
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

pub fn lower_typed_function_call_emission_plans(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    plans: &TypedCallEmissionPlans,
) -> Result<usize, String> {
    if plans.is_empty() {
        return Ok(0);
    }

    struct Rewriter<'a> {
        plans: &'a TypedCallEmissionPlans,
        count: usize,
        error: Option<String>,
    }

    impl VisitMut<InstrTyped> for Rewriter<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            expr.visit_children_mut(self);
            let InstrTyped::CallTyped(call) = expr else {
                return;
            };
            let Some(instr_id) = call.try_semantic_instr_id() else {
                return;
            };
            let Some(plan) = self.plans.by_source.get(&instr_id) else {
                return;
            };
            if plan.is_empty() {
                return;
            }

            let old_expr = std::mem::replace(expr, InstrTyped::constant_none());
            let InstrTyped::CallTyped(call) = old_expr else {
                unreachable!("checked call shape before replacing typed instruction")
            };
            match plan {
                TypedCallEmissionPlan::Callable {
                    function_guards,
                    constructor_guards,
                } => {
                    *expr = InstrTyped::GuardedCallableCallTyped(
                        TypedGuardedCallableCall::from_typed_call(
                            call,
                            function_guards.clone(),
                            constructor_guards.clone(),
                        ),
                    );
                    self.count += 1;
                }
                TypedCallEmissionPlan::Method {
                    method_name,
                    method_guards,
                } => {
                    *expr = InstrTyped::GuardedMethodCallTyped(
                        TypedGuardedMethodCall::from_typed_call(
                            call,
                            method_name.clone(),
                            method_guards.clone(),
                        ),
                    );
                    self.count += 1;
                }
            }
        }
    }

    let mut rewriter = Rewriter {
        plans,
        count: 0,
        error: None,
    };
    rewriter.visit_fn_mut(function);
    if let Some(err) = rewriter.error {
        return Err(err);
    }
    Ok(rewriter.count)
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

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct TypedInlineRewriteStats {
    pub rewritten_stores: usize,
    pub skipped_candidates: usize,
    pub skipped_exception_edges: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TypedInlineUnsupportedReason {
    MissingCallerStorageLayout,
    MissingCalleeStorageLayout,
    MissingCalleeLocal(LocalLocation),
    MissingParameterLocal,
    RebindsBoundLocal(LocalLocation),
    ArityMismatch,
    KeywordArguments,
    StarredArguments,
    DefaultArguments,
    UnsupportedParameterKind,
    TooManyBlocks,
    MultipleBlocks,
    UnknownLabel(BlockLabel),
    BlockParams,
    JumpArgs,
    ExceptionEdge,
    NonReturnTerm,
}

#[derive(Clone)]
struct TypedInlineDirectCallPlan {
    target: RuntimeFunctionId,
    arg_plan: TypedDirectCallArgPlan,
}

pub fn inline_typed_function_direct_call_stores(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    module: &BlockPyModule<TypedCodegenModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, BlockPyFunction<TypedCodegenModuleShape>>,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> TypedInlineRewriteStats {
    if direct_calls_by_instr_id.is_empty() {
        return TypedInlineRewriteStats::default();
    }

    let mut stats = TypedInlineRewriteStats::default();
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match build_typed_direct_call_inline_rewrite(
            function,
            module,
            external_callees,
            block,
            direct_calls_by_instr_id,
            &mut stats,
        ) {
            TypedInlineBlockRewrite::Rewritten(blocks) => {
                stats.rewritten_stores += 1;
                rewritten_blocks.extend(blocks);
            }
            TypedInlineBlockRewrite::Unchanged(block) => rewritten_blocks.push(block),
        }
    }
    function.blocks = rewritten_blocks;
    stats
}

enum TypedInlineBlockRewrite {
    Rewritten(Vec<TypedBlock>),
    Unchanged(TypedBlock),
}

struct TypedInlineStoreCandidate {
    instr_index: usize,
    target: ResolvedName,
    call: TypedGuardedCallableCall<InstrTyped>,
    inline_plans: Vec<TypedInlineDirectCallPlan>,
}

fn build_typed_direct_call_inline_rewrite(
    caller: &mut BlockPyFunction<TypedCodegenModuleShape>,
    module: &BlockPyModule<TypedCodegenModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, BlockPyFunction<TypedCodegenModuleShape>>,
    block: TypedBlock,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
    stats: &mut TypedInlineRewriteStats,
) -> TypedInlineBlockRewrite {
    let original_block = block.clone();
    let original_storage_layout = caller.storage_layout.clone();
    let Some(candidate) =
        find_typed_inline_store_candidate(&block, caller.function_id, direct_calls_by_instr_id)
    else {
        return TypedInlineBlockRewrite::Unchanged(block);
    };
    if block.exc_edge.is_some() {
        stats.skipped_exception_edges += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    }
    if !candidate.call.keywords.is_empty() {
        stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    }
    let Some(positional_arg_exprs) = typed_positional_arg_exprs(candidate.call.args.clone()) else {
        stats.skipped_candidates += 1;
        return TypedInlineBlockRewrite::Unchanged(block);
    };

    let callable_temp = match try_allocate_typed_stack_temp(caller, "typed_inline_callable") {
        Ok(temp) => temp,
        Err(_) => {
            stats.skipped_candidates += 1;
            return TypedInlineBlockRewrite::Unchanged(block);
        }
    };
    let arg_temps = match (0..positional_arg_exprs.len())
        .map(|_| try_allocate_typed_stack_temp(caller, "typed_inline_arg"))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(temps) => temps,
        Err(_) => {
            stats.skipped_candidates += 1;
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(block);
        }
    };

    let continuation_label = caller.name_gen.next_block_name();
    let generic_label = caller.name_gen.next_block_name();
    let cleanup_label = caller.name_gen.next_block_name();
    let guard_labels = (0..candidate.inline_plans.len().saturating_sub(1))
        .map(|_| caller.name_gen.next_block_name())
        .collect::<Vec<_>>();
    let hot_labels = candidate
        .inline_plans
        .iter()
        .map(|_| caller.name_gen.next_block_name())
        .collect::<Vec<_>>();

    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    before.truncate(candidate.instr_index);
    before.push(
        Store::new(callable_temp.resolved_name(), *candidate.call.func.clone())
            .with_meta(Meta::synthetic())
            .into(),
    );
    for (arg_temp, arg_expr) in arg_temps.iter().zip(positional_arg_exprs) {
        before.push(
            Store::new(arg_temp.resolved_name(), arg_expr)
                .with_meta(Meta::synthetic())
                .into(),
        );
    }

    let entry = Block::new_with_extra(
        block.label,
        before,
        typed_direct_call_guard_term(
            &callable_temp.resolved_name(),
            candidate.inline_plans[0].target,
            hot_labels[0],
            guard_labels.first().copied().unwrap_or(generic_label),
        ),
        block.params,
        None,
        block.extra,
    );

    let mut blocks: Vec<TypedBlock> = Vec::new();
    blocks.push(entry);

    for (guard_index, guard_label) in guard_labels.iter().copied().enumerate() {
        let target_index = guard_index + 1;
        let else_label = guard_labels
            .get(guard_index + 1)
            .copied()
            .unwrap_or(generic_label);
        blocks.push(Block::new_with_extra(
            guard_label,
            Vec::new(),
            typed_direct_call_guard_term(
                &callable_temp.resolved_name(),
                candidate.inline_plans[target_index].target,
                hot_labels[target_index],
                else_label,
            ),
            Vec::new(),
            None,
            TypedBlockExtra::default(),
        ));
    }

    for (plan, hot_label) in candidate.inline_plans.iter().zip(hot_labels) {
        let Some(callee) = typed_inline_callee(module, external_callees, plan.target) else {
            stats.skipped_candidates += 1;
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        let Ok(bindings) = bind_typed_direct_call_inline_args(callee, &plan.arg_plan, &arg_temps)
        else {
            stats.skipped_candidates += 1;
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        let Ok(mut fragment) = build_typed_direct_call_inline_fragment_to_target(
            caller,
            callee,
            cleanup_label,
            &bindings,
            candidate.target.clone(),
        ) else {
            stats.skipped_candidates += 1;
            caller.storage_layout = original_storage_layout;
            return TypedInlineBlockRewrite::Unchanged(original_block);
        };
        if let Some(entry) = fragment.blocks.first_mut() {
            entry.label = hot_label;
        }
        blocks.extend(fragment.blocks);
    }

    blocks.push(Block::new_with_extra(
        generic_label,
        typed_generic_call_fallback_body(
            &candidate.target,
            &callable_temp.resolved_name(),
            &arg_temps,
        ),
        BlockTerm::Jump(BlockEdge::new(continuation_label)),
        Vec::new(),
        None,
        TypedBlockExtra::default(),
    ));

    let mut cleanup_body = Vec::new();
    append_typed_cleanup_dels_to_body(&mut cleanup_body, &arg_temps);
    append_typed_cleanup_del_to_body(&mut cleanup_body, &callable_temp.resolved_name());
    blocks.push(Block::new_with_extra(
        cleanup_label,
        cleanup_body,
        BlockTerm::Jump(BlockEdge::new(continuation_label)),
        Vec::new(),
        None,
        TypedBlockExtra::default(),
    ));
    blocks.push(Block::new_with_extra(
        continuation_label,
        after,
        block.term,
        Vec::new(),
        None,
        TypedBlockExtra::default(),
    ));

    TypedInlineBlockRewrite::Rewritten(blocks)
}

fn find_typed_inline_store_candidate(
    block: &TypedBlock,
    caller_id: RuntimeFunctionId,
    direct_calls_by_instr_id: &HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>,
) -> Option<TypedInlineStoreCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let InstrTyped::GuardedCallableCallTyped(call) = store.value.as_ref() else {
                return None;
            };
            let instr_id = call.try_semantic_instr_id()?;
            let plans = direct_calls_by_instr_id.get(&instr_id)?;
            let inline_plans = plans
                .iter()
                .filter_map(|(target, arg_plan)| {
                    if *target == caller_id
                        || !call
                            .function_guards
                            .iter()
                            .any(|guard| guard.function_id == *target)
                    {
                        return None;
                    }
                    Some(TypedInlineDirectCallPlan {
                        target: *target,
                        arg_plan: arg_plan.clone(),
                    })
                })
                .collect::<Vec<_>>();
            (!inline_plans.is_empty()).then_some(TypedInlineStoreCandidate {
                instr_index,
                target: store.name.clone(),
                call: call.clone(),
                inline_plans,
            })
        })
}

fn typed_inline_callee<'a>(
    module: &'a BlockPyModule<TypedCodegenModuleShape>,
    external_callees: &'a HashMap<RuntimeFunctionId, BlockPyFunction<TypedCodegenModuleShape>>,
    function_id: RuntimeFunctionId,
) -> Option<&'a BlockPyFunction<TypedCodegenModuleShape>> {
    module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .or_else(|| external_callees.get(&function_id))
}

fn typed_positional_arg_exprs(args: Vec<CallArgPositional<InstrTyped>>) -> Option<Vec<InstrTyped>> {
    args.into_iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(expr) => Some(expr),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

fn typed_direct_call_guard_term(
    callable_temp: &ResolvedName,
    function_id: RuntimeFunctionId,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrTyped> {
    BlockTerm::IfTerm(TermIf {
        test: InstrTyped::DirectCallGuardTest(TypedDirectCallGuardTest::new(
            typed_load_temp(callable_temp),
            TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id },
        )),
        then_label,
        else_label,
    })
}

fn typed_generic_call_fallback_body(
    target: &ResolvedName,
    callable_temp: &ResolvedName,
    arg_temps: &[TypedTempLocal],
) -> Vec<InstrTyped> {
    let mut body = vec![
        Store::new(
            target.clone(),
            Box::new(InstrTyped::CallTyped(TypedCall::generic(
                typed_load_temp(callable_temp),
                typed_load_temp_args(arg_temps),
                Vec::<CallArgKeyword<InstrTyped>>::new(),
            ))),
        )
        .with_meta(Meta::synthetic())
        .into(),
    ];
    append_typed_cleanup_dels_to_body(&mut body, arg_temps);
    append_typed_cleanup_del_to_body(&mut body, callable_temp);
    body
}

fn typed_load_temp(temp_name: &ResolvedName) -> InstrTyped {
    InstrTyped::Load(Load::new(temp_name.clone()).with_meta(Meta::synthetic()))
}

fn typed_load_temp_args(temp_names: &[TypedTempLocal]) -> Vec<CallArgPositional<InstrTyped>> {
    temp_names
        .iter()
        .map(|temp| CallArgPositional::Positional(typed_load_temp(&temp.resolved_name())))
        .collect()
}

fn append_typed_cleanup_dels_to_body(body: &mut Vec<InstrTyped>, temp_names: &[TypedTempLocal]) {
    for temp_name in temp_names.iter().rev() {
        append_typed_cleanup_del_to_body(body, &temp_name.resolved_name());
    }
}

fn append_typed_cleanup_del_to_body(body: &mut Vec<InstrTyped>, temp_name: &ResolvedName) {
    body.push(
        Del::new(temp_name.clone(), false)
            .with_meta(Meta::synthetic())
            .into(),
    );
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TypedTempLocal {
    name: String,
    location: LocalLocation,
}

impl TypedTempLocal {
    fn resolved_name(&self) -> ResolvedName {
        ResolvedName {
            id: self.name.clone().into(),
            location: NameLocation::Local(self.location),
        }
    }
}

fn try_allocate_typed_stack_temp(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    prefix: &str,
) -> Result<TypedTempLocal, TypedInlineUnsupportedReason> {
    let name = function.name_gen.next_tmp_name(prefix).as_str().to_string();
    let layout = function
        .storage_layout
        .as_mut()
        .ok_or(TypedInlineUnsupportedReason::MissingCallerStorageLayout)?;
    let location = LocalLocation(
        u32::try_from(layout.stack_slots().len())
            .expect("typed stack slot index should fit in u32"),
    );
    layout.ensure_stack_slot(name.clone());
    Ok(TypedTempLocal { name, location })
}

type TypedInlineValueBindings = HashMap<LocalLocation, InstrTyped>;

fn bind_typed_direct_call_inline_args(
    callee: &BlockPyFunction<TypedCodegenModuleShape>,
    arg_plan: &TypedDirectCallArgPlan,
    arg_temps: &[TypedTempLocal],
) -> Result<TypedInlineValueBindings, TypedInlineUnsupportedReason> {
    if arg_plan.sources.len() != callee.params.len() {
        return Err(TypedInlineUnsupportedReason::ArityMismatch);
    }
    let mut bindings = TypedInlineValueBindings::new();
    for (param, source) in callee.params.iter().zip(&arg_plan.sources) {
        if !matches!(param.kind, ParamKind::PosOnly | ParamKind::Any) {
            return Err(TypedInlineUnsupportedReason::UnsupportedParameterKind);
        }
        let TypedDirectCallArgSource::Provided(index) = source else {
            return Err(TypedInlineUnsupportedReason::DefaultArguments);
        };
        let Some(arg_temp) = arg_temps.get(*index) else {
            return Err(TypedInlineUnsupportedReason::ArityMismatch);
        };
        let location = typed_parameter_local_location(callee, &param.name)?;
        bindings.insert(location, typed_load_temp(&arg_temp.resolved_name()));
    }
    Ok(bindings)
}

fn typed_parameter_local_location(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    name: &str,
) -> Result<LocalLocation, TypedInlineUnsupportedReason> {
    let layout = function
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    let Some(slot) = layout
        .stack_slots()
        .iter()
        .position(|slot_name| slot_name == name)
    else {
        return Err(TypedInlineUnsupportedReason::MissingParameterLocal);
    };
    Ok(LocalLocation(
        u32::try_from(slot).expect("parameter stack slot index should fit in u32"),
    ))
}

fn build_typed_direct_call_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedCodegenModuleShape>,
    callee: &BlockPyFunction<TypedCodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    return_target: ResolvedName,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    if callee.blocks.len() == 1 {
        return build_single_block_typed_inline_fragment_to_target(
            caller,
            callee,
            continuation,
            value_bindings,
            return_target,
        );
    }
    build_multi_block_typed_inline_fragment_to_target(
        caller,
        callee,
        continuation,
        value_bindings,
        return_target,
    )
}

struct TypedInlineFragment {
    blocks: Vec<TypedBlock>,
}

fn build_single_block_typed_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedCodegenModuleShape>,
    callee: &BlockPyFunction<TypedCodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    return_target: ResolvedName,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    if callee.blocks.len() != 1 {
        return Err(TypedInlineUnsupportedReason::MultipleBlocks);
    }
    let callee_block = &callee.blocks[0];
    if !callee_block.params.is_empty() {
        return Err(TypedInlineUnsupportedReason::BlockParams);
    }
    if callee_block.exc_edge.is_some() {
        return Err(TypedInlineUnsupportedReason::ExceptionEdge);
    }
    let BlockTerm::Return(return_value) = &callee_block.term else {
        return Err(TypedInlineUnsupportedReason::NonReturnTerm);
    };

    let locals = allocate_typed_inline_locals(caller, callee_layout, value_bindings)?;
    let mut remapper = TypedInlineLocalRemapper::new(&locals, value_bindings);
    let mut body = callee_block
        .body
        .iter()
        .cloned()
        .filter(|instr| !matches!(instr, InstrTyped::IncrementCounter(_)))
        .map(|instr| remapper.try_map_instr(instr))
        .collect::<Result<Vec<_>, _>>()?;
    let return_value = remapper.try_map_instr(return_value.clone())?;
    let return_meta = return_value.meta();
    body.push(
        Store::new(return_target, Box::new(return_value))
            .with_meta(return_meta)
            .into(),
    );

    Ok(TypedInlineFragment {
        blocks: vec![Block::new_with_extra(
            caller.name_gen.next_block_name(),
            body,
            BlockTerm::Jump(BlockEdge::new(continuation)),
            Vec::new(),
            None,
            TypedBlockExtra::default(),
        )],
    })
}

fn build_multi_block_typed_inline_fragment_to_target(
    caller: &mut BlockPyFunction<TypedCodegenModuleShape>,
    callee: &BlockPyFunction<TypedCodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &TypedInlineValueBindings,
    return_target: ResolvedName,
) -> Result<TypedInlineFragment, TypedInlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(TypedInlineUnsupportedReason::MissingCalleeStorageLayout)?;
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    for block in &callee.blocks {
        if !block.params.is_empty() {
            return Err(TypedInlineUnsupportedReason::BlockParams);
        }
        if block.exc_edge.is_some() {
            return Err(TypedInlineUnsupportedReason::ExceptionEdge);
        }
        if typed_term_has_jump_args(&block.term) {
            return Err(TypedInlineUnsupportedReason::JumpArgs);
        }
    }

    let locals = allocate_typed_inline_locals(caller, callee_layout, value_bindings)?;
    let label_map = callee
        .blocks
        .iter()
        .map(|block| (block.label, caller.name_gen.next_block_name()))
        .collect::<HashMap<_, _>>();
    let mut remapper = TypedInlineLocalRemapper::new(&locals, value_bindings);
    let mut blocks: Vec<TypedBlock> = Vec::with_capacity(callee.blocks.len());
    for callee_block in &callee.blocks {
        let label = typed_remapped_label(&label_map, callee_block.label)?;
        let mut body = callee_block
            .body
            .iter()
            .cloned()
            .filter(|instr| !matches!(instr, InstrTyped::IncrementCounter(_)))
            .map(|instr| remapper.try_map_instr(instr))
            .collect::<Result<Vec<_>, _>>()?;
        let term = match &callee_block.term {
            BlockTerm::Return(value) => {
                let return_value = remapper.try_map_instr(value.clone())?;
                let return_meta = return_value.meta();
                body.push(
                    Store::new(return_target.clone(), Box::new(return_value))
                        .with_meta(return_meta)
                        .into(),
                );
                BlockTerm::Jump(BlockEdge::new(continuation))
            }
            term => {
                typed_remap_inline_term_labels(remapper.try_map_term(term.clone())?, &label_map)?
            }
        };
        blocks.push(Block::new_with_extra(
            label,
            body,
            term,
            Vec::new(),
            None,
            callee_block.extra.clone(),
        ));
    }
    Ok(TypedInlineFragment { blocks })
}

fn allocate_typed_inline_locals(
    caller: &mut BlockPyFunction<TypedCodegenModuleShape>,
    callee_layout: &soac_core::block_py::StorageLayout,
    value_bindings: &TypedInlineValueBindings,
) -> Result<HashMap<LocalLocation, TypedTempLocal>, TypedInlineUnsupportedReason> {
    let mut locals = HashMap::new();
    for (slot, _name) in callee_layout.stack_slots().iter().enumerate() {
        let location =
            LocalLocation(u32::try_from(slot).expect("callee stack slot index should fit in u32"));
        if value_bindings.contains_key(&location) {
            continue;
        }
        locals.insert(
            location,
            try_allocate_typed_stack_temp(caller, "typed_inline")?,
        );
    }
    Ok(locals)
}

fn typed_term_has_jump_args(term: &BlockTerm<InstrTyped>) -> bool {
    match term {
        BlockTerm::Jump(edge) => !edge.args.is_empty(),
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_)
        | BlockTerm::Return(_) => false,
    }
}

fn typed_remapped_label(
    label_map: &HashMap<BlockLabel, BlockLabel>,
    label: BlockLabel,
) -> Result<BlockLabel, TypedInlineUnsupportedReason> {
    label_map
        .get(&label)
        .copied()
        .ok_or(TypedInlineUnsupportedReason::UnknownLabel(label))
}

fn typed_remap_inline_term_labels(
    term: BlockTerm<InstrTyped>,
    label_map: &HashMap<BlockLabel, BlockLabel>,
) -> Result<BlockTerm<InstrTyped>, TypedInlineUnsupportedReason> {
    Ok(match term {
        BlockTerm::Jump(edge) => BlockTerm::Jump(BlockEdge::new(typed_remapped_label(
            label_map,
            edge.target,
        )?)),
        BlockTerm::IfTerm(mut term) => {
            term.then_label = typed_remapped_label(label_map, term.then_label)?;
            term.else_label = typed_remapped_label(label_map, term.else_label)?;
            BlockTerm::IfTerm(term)
        }
        BlockTerm::BranchTable(mut term) => {
            for target in &mut term.targets {
                *target = typed_remapped_label(label_map, *target)?;
            }
            term.default_label = typed_remapped_label(label_map, term.default_label)?;
            BlockTerm::BranchTable(term)
        }
        BlockTerm::Raise(term) => BlockTerm::Raise(term),
        BlockTerm::Return(_) => return Err(TypedInlineUnsupportedReason::NonReturnTerm),
    })
}

struct TypedInlineLocalRemapper<'locals, 'bindings> {
    locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
    value_bindings: &'bindings TypedInlineValueBindings,
}

impl<'locals, 'bindings> TypedInlineLocalRemapper<'locals, 'bindings> {
    fn new(
        locals: &'locals HashMap<LocalLocation, TypedTempLocal>,
        value_bindings: &'bindings TypedInlineValueBindings,
    ) -> Self {
        Self {
            locals,
            value_bindings,
        }
    }
}

impl TryMapInstr<InstrTyped, InstrTyped, TypedInlineUnsupportedReason>
    for TypedInlineLocalRemapper<'_, '_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrTyped,
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        let mapped = match instr {
            InstrTyped::Truthy(op) => InstrTyped::Truthy(op.try_map_children(self)?),
            InstrTyped::Load(op) => {
                if let Some(location) = op.name.local_location()
                    && let Some(value) = self.value_bindings.get(&location)
                {
                    return Ok(clear_typed_instr_ids(value.clone()));
                }
                InstrTyped::Load(op.try_map_children(self)?)
            }
            InstrTyped::BinOp(op) => InstrTyped::BinOp(op.try_map_children(self)?),
            InstrTyped::Tuple(op) => InstrTyped::Tuple(op.try_map_children(self)?),
            InstrTyped::UnaryOp(op) => InstrTyped::UnaryOp(op.try_map_children(self)?),
            InstrTyped::CalleeFunctionId(op) => {
                InstrTyped::CalleeFunctionId(op.try_map_children(self)?)
            }
            InstrTyped::CallTyped(op) => InstrTyped::CallTyped(op.try_map_children(self)?),
            InstrTyped::GuardedCallableCallTyped(op) => {
                InstrTyped::GuardedCallableCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::GuardedMethodCallTyped(op) => {
                InstrTyped::GuardedMethodCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::DirectCallableCallTyped(op) => {
                InstrTyped::DirectCallableCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::DirectMethodCallTyped(op) => {
                InstrTyped::DirectMethodCallTyped(op.try_map_children(self)?)
            }
            InstrTyped::DirectCallGuardTest(op) => {
                InstrTyped::DirectCallGuardTest(op.try_map_children(self)?)
            }
            InstrTyped::CallDirect(op) => InstrTyped::CallDirect(op.try_map_children(self)?),
            InstrTyped::GetAttrTyped(op) => InstrTyped::GetAttrTyped(op.try_map_children(self)?),
            InstrTyped::SetAttrTyped(op) => InstrTyped::SetAttrTyped(op.try_map_children(self)?),
            InstrTyped::GetItem(op) => InstrTyped::GetItem(op.try_map_children(self)?),
            InstrTyped::SetItem(op) => InstrTyped::SetItem(op.try_map_children(self)?),
            InstrTyped::DelItem(op) => InstrTyped::DelItem(op.try_map_children(self)?),
            InstrTyped::Store(op) => {
                if let Some(location) = op.name.local_location()
                    && self.value_bindings.contains_key(&location)
                {
                    return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
                }
                InstrTyped::Store(op.try_map_children(self)?)
            }
            InstrTyped::Del(op) => {
                if let Some(location) = op.name.local_location()
                    && self.value_bindings.contains_key(&location)
                {
                    return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
                }
                InstrTyped::Del(op.try_map_children(self)?)
            }
            InstrTyped::MakeCell(op) => InstrTyped::MakeCell(op.try_map_children(self)?),
            InstrTyped::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrTyped::CellRef(op) => InstrTyped::CellRef(op),
            InstrTyped::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
        };
        Ok(clear_typed_instr_id(mapped))
    }

    fn try_map_name(
        &mut self,
        mut name: ResolvedName,
    ) -> Result<ResolvedName, TypedInlineUnsupportedReason> {
        let Some(location) = name.location.as_local() else {
            return Ok(name);
        };
        if self.value_bindings.contains_key(&location) {
            return Err(TypedInlineUnsupportedReason::RebindsBoundLocal(location));
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(TypedInlineUnsupportedReason::MissingCalleeLocal(location));
        };
        name.id = fresh.name.clone().into();
        name.location = NameLocation::Local(fresh.location);
        Ok(name)
    }
}

fn clear_typed_instr_ids(mut instr: InstrTyped) -> InstrTyped {
    struct Scrubber;
    impl VisitMut<InstrTyped> for Scrubber {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            let mut meta = expr.meta();
            meta.instr_id = None;
            *expr = expr.clone().with_meta(meta);
        }
    }
    Scrubber.visit_instr_mut(&mut instr);
    instr
}

fn clear_typed_instr_id(instr: InstrTyped) -> InstrTyped {
    let mut meta = instr.meta();
    meta.instr_id = None;
    instr.with_meta(meta)
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
            if !matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_)) {
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
            InstrTyped::Load(op) => InstrCodegen::Load(op.try_map_children(self)?),
            InstrTyped::BinOp(op) => InstrCodegen::BinOp(op.try_map_children(self)?),
            InstrTyped::Tuple(op) => InstrCodegen::Tuple(op.try_map_children(self)?),
            InstrTyped::UnaryOp(op) => InstrCodegen::UnaryOp(op.try_map_children(self)?),
            InstrTyped::CalleeFunctionId(op) => {
                InstrCodegen::CalleeFunctionId(op.try_map_children(self)?)
            }
            InstrTyped::DirectCallGuardTest(op) => {
                let meta = op.meta();
                match op.kind {
                    TypedDirectCallGuardTestKind::RuntimeFunctionId { function_id } => {
                        InstrCodegen::DirectFunctionIdGuardTest(
                            DirectFunctionIdGuardTest::new(
                                self.try_map_instr(*op.value)?,
                                function_id,
                            )
                            .with_meta(meta),
                        )
                    }
                }
            }
            InstrTyped::CallTyped(op) => {
                InstrCodegen::Call(op.try_map_children(self)?.into_legacy())
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
            InstrTyped::CallDirect(op) => InstrCodegen::CallDirect(op.try_map_children(self)?),
            InstrTyped::GetAttrTyped(op) => {
                InstrCodegen::GetAttr(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::SetAttrTyped(op) => {
                InstrCodegen::SetAttr(op.try_map_children(self)?.into_legacy())
            }
            InstrTyped::GetItem(op) => InstrCodegen::GetItem(op.try_map_children(self)?),
            InstrTyped::SetItem(op) => InstrCodegen::SetItem(op.try_map_children(self)?),
            InstrTyped::DelItem(op) => InstrCodegen::DelItem(op.try_map_children(self)?),
            InstrTyped::Store(op) => InstrCodegen::Store(op.try_map_children(self)?),
            InstrTyped::Del(op) => InstrCodegen::Del(op.try_map_children(self)?),
            InstrTyped::MakeCell(op) => InstrCodegen::MakeCell(op.try_map_children(self)?),
            InstrTyped::IncrementCounter(op) => InstrCodegen::IncrementCounter(op),
            InstrTyped::CellRef(op) => InstrCodegen::CellRef(op),
            InstrTyped::MakeFunctionWithClosure(op) => {
                InstrCodegen::MakeFunctionWithClosure(op.try_map_children(self)?)
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

#[derive(Debug, Clone)]
pub struct TypedCodegenModuleShape;

impl ModuleShape for TypedCodegenModuleShape {
    type Instr = InstrTyped;
    type ModuleConstant = InstrResolved;
    type BlockExtra = TypedBlockExtra;
}

#[cfg(test)]
mod typed_codegen_tests {
    use super::*;
    use crate::passes::{TypedAttrOwnerRef, infer_module_value_facts};
    use soac_core::block_py::{ChildVisitable, InstrId, Visit, VisitMut};
    use std::collections::HashMap;

    #[derive(Default)]
    struct TypedInstrCounter {
        total: usize,
        truthy: usize,
        loads: usize,
        binops: usize,
        tuples: usize,
        unary_ops: usize,
        typed_calls: usize,
        guarded_callable_calls: usize,
        guarded_method_calls: usize,
        direct_callable_calls: usize,
        direct_method_calls: usize,
        direct_call_guard_tests: usize,
        typed_getattrs: usize,
        typed_setattrs: usize,
        getitems: usize,
        setitems: usize,
        delitems: usize,
        stores: usize,
        dels: usize,
        make_cells: usize,
        increment_counters: usize,
        cell_refs: usize,
        make_functions_with_closure: usize,
        first_class: usize,
    }

    impl Visit<InstrTyped> for TypedInstrCounter {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.total += 1;
            if matches!(expr, InstrTyped::Truthy(_)) {
                self.truthy += 1;
            }
            if matches!(expr, InstrTyped::Load(_)) {
                self.loads += 1;
            }
            self.first_class += 1;
            if matches!(expr, InstrTyped::BinOp(_)) {
                self.binops += 1;
            }
            if matches!(expr, InstrTyped::Tuple(_)) {
                self.tuples += 1;
            }
            if matches!(expr, InstrTyped::UnaryOp(_)) {
                self.unary_ops += 1;
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
            if matches!(expr, InstrTyped::GetItem(_)) {
                self.getitems += 1;
            }
            if matches!(expr, InstrTyped::SetItem(_)) {
                self.setitems += 1;
            }
            if matches!(expr, InstrTyped::DelItem(_)) {
                self.delitems += 1;
            }
            if matches!(expr, InstrTyped::Store(_)) {
                self.stores += 1;
            }
            if matches!(expr, InstrTyped::Del(_)) {
                self.dels += 1;
            }
            if matches!(expr, InstrTyped::MakeCell(_)) {
                self.make_cells += 1;
            }
            if matches!(expr, InstrTyped::IncrementCounter(_)) {
                self.increment_counters += 1;
            }
            if matches!(expr, InstrTyped::CellRef(_)) {
                self.cell_refs += 1;
            }
            if matches!(expr, InstrTyped::MakeFunctionWithClosure(_)) {
                self.make_functions_with_closure += 1;
            }
            expr.visit_children(self);
        }
    }

    fn first_class_count(counter: &TypedInstrCounter) -> usize {
        counter.truthy
            + counter.loads
            + counter.binops
            + counter.tuples
            + counter.unary_ops
            + counter.typed_calls
            + counter.guarded_callable_calls
            + counter.guarded_method_calls
            + counter.direct_callable_calls
            + counter.direct_method_calls
            + counter.direct_call_guard_tests
            + counter.typed_getattrs
            + counter.typed_setattrs
            + counter.getitems
            + counter.setitems
            + counter.delitems
            + counter.stores
            + counter.dels
            + counter.make_cells
            + counter.increment_counters
            + counter.cell_refs
            + counter.make_functions_with_closure
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

    fn first_typed_call_instr_id(function: &BlockPyFunction<TypedCodegenModuleShape>) -> InstrId {
        struct Finder {
            instr_id: Option<InstrId>,
        }

        impl Visit<InstrTyped> for Finder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if self.instr_id.is_none() {
                    if let InstrTyped::CallTyped(call) = expr {
                        self.instr_id = call.try_semantic_instr_id();
                        return;
                    }
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder { instr_id: None };
        finder.visit_fn(function);
        finder
            .instr_id
            .expect("test function should contain a typed call with an instruction id")
    }

    #[test]
    fn lower_codegen_module_to_typed_keeps_loads_first_class() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f(a, b):\n    return a + b\n")
                .expect("source should lower");
        let function_count = lowered.codegen_module.callable_defs.len();
        let global_names = lowered.codegen_module.global_names.clone();

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        assert_eq!(typed.callable_defs.len(), function_count);
        assert_eq!(typed.global_names, global_names);

        let mut counter = TypedInstrCounter::default();
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
        assert_eq!(counter.first_class, first_class_count(&counter));
    }

    #[test]
    fn typed_instr_extras_start_without_result_facts() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
                .expect("source should lower");

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let counter = count_typed_extra_facts(&typed);

        assert!(counter.extras > 0);
        assert_eq!(counter.facts, 0);
    }

    #[test]
    fn annotate_typed_module_value_facts_materializes_result_facts() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
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
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return None\n")
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
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
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

        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def f():\n    return 1 + 2\n")
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
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(obj, value):\n    obj.x = value\n    return obj.x\n",
        )
        .expect("source should lower");

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let mut counter = TypedInstrCounter::default();
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
    fn lower_codegen_module_to_typed_makes_core_ops_first_class() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def outer(seq, value):
    x = (-value, value)
    seq[0] = value
    y = seq[0]
    del seq[0]
    del y
    def inner():
        return x
    return inner
"#,
        )
        .expect("source should lower");

        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let mut counter = TypedInstrCounter::default();
        for function in &typed.callable_defs {
            counter.visit_fn(function);
        }

        assert!(
            counter.tuples > 0,
            "tuple ops should be first-class typed ops"
        );
        assert!(
            counter.unary_ops > 0,
            "unary ops should be first-class typed ops"
        );
        assert!(
            counter.getitems > 0,
            "getitem ops should be first-class typed ops"
        );
        assert!(
            counter.setitems > 0,
            "setitem ops should be first-class typed ops"
        );
        assert!(
            counter.delitems > 0,
            "delitem ops should be first-class typed ops"
        );
        assert!(
            counter.stores > 0,
            "store ops should be first-class typed ops"
        );
        assert!(counter.dels > 0, "del ops should be first-class typed ops");
        assert!(
            counter.make_cells > 0,
            "make-cell ops should be first-class typed ops"
        );
        assert!(
            counter.cell_refs > 0,
            "cell-ref ops should be first-class typed ops"
        );
        assert!(
            counter.make_functions_with_closure > 0,
            "closure function creation should be a first-class typed op"
        );
    }

    #[test]
    fn typed_legacy_module_round_trips_to_codegen_shape() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(a, b):\n    return g(a + b)\n",
        )
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
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def f(x):\n    if x:\n        return 1\n    return 0\n",
        )
        .expect("source should lower");
        let typed = lower_codegen_module_to_typed(lowered.codegen_module);

        let typed = lower_typed_if_tests_to_truthy(typed);

        let mut counter = TypedInstrCounter::default();
        for function in &typed.callable_defs {
            for block in &function.blocks {
                if let BlockTerm::IfTerm(if_term) = &block.term {
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
        assert_eq!(counter.first_class, first_class_count(&counter));
        assert!(
            try_lower_typed_module_to_codegen_legacy(typed).is_err(),
            "typed truthiness should not silently lower through the legacy adapter"
        );
    }

    #[test]
    fn validates_guarded_method_typed_call_access_plan_shape() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
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
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
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

        let mut counter = TypedInstrCounter::default();
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
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
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

        let mut counter = TypedInstrCounter::default();
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
    fn lowers_typed_call_emission_plan_to_guarded_callable_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def add(a):\n    return a\n\n\
def caller(a):\n    return add(a)\n",
        )
        .expect("source should lower");
        let add_id = codegen_function_id_by_qualname(&lowered.codegen_module, "add");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Callable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: add_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                    constructor_guards: Vec::new(),
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed call emission plan should lower"),
            1
        );
        assert_eq!(
            lower_typed_function_call_access_plan_instrs(caller),
            0,
            "mechanical call emission lowering should not round-trip through access plans"
        );

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_callable_calls, 1);
    }

    #[test]
    fn lowers_typed_call_emission_plan_with_function_and_constructor_guards() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(a):\n    return callable(a)\n",
        )
        .expect("source should lower");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let direct_target = RuntimeFunctionId::from_raw_parts(0, 7);
        let constructor_target = RuntimeFunctionId::from_raw_parts(0, 8);
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![TypedDirectCallArgSource::Provided(0)],
        };
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Callable {
                    function_guards: vec![TypedDirectFunctionCallGuard {
                        function_id: direct_target,
                        arg_plan: arg_plan.clone(),
                    }],
                    constructor_guards: vec![TypedDirectConstructorCallGuard {
                        function_id: constructor_target,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "test".to_string(),
                            qualname: "Box".to_string(),
                        },
                        type_version: 11,
                        arg_plan,
                    }],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed callable emission plan should lower"),
            1
        );
        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.guarded_callable_calls, 1);
    }

    #[test]
    fn lowers_typed_call_emission_plan_to_guarded_method_instr() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(box):\n    return box.get(1)\n",
        )
        .expect("source should lower");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let target = RuntimeFunctionId::from_raw_parts(0, 9);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Method {
                    method_name: "get".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: target,
                        owner_type_ref: TypedAttrOwnerRef::TypeKey {
                            module_name: "test".to_string(),
                            qualname: "Box".to_string(),
                        },
                        type_version: 11,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("typed method emission plan should lower"),
            1
        );
        assert_eq!(
            lower_typed_function_call_access_plan_instrs(caller),
            0,
            "mechanical method emission lowering should not round-trip through access plans"
        );

        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 0);
        assert_eq!(counter.guarded_method_calls, 1);
    }

    #[test]
    fn empty_typed_call_emission_plan_leaves_generic_call_in_place() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(box):\n    return box.get(1)\n",
        )
        .expect("source should lower");
        let mut typed = lower_codegen_module_to_typed(lowered.codegen_module);
        let caller = typed_function_by_qualname_mut(&mut typed, "caller");
        let call_id = first_typed_call_instr_id(caller);
        let plans = TypedCallEmissionPlans {
            by_source: HashMap::from([(
                call_id,
                TypedCallEmissionPlan::Method {
                    method_name: "get".to_string(),
                    method_guards: Vec::new(),
                },
            )]),
        };

        assert_eq!(
            lower_typed_function_call_emission_plans(caller, &plans)
                .expect("empty typed emission plan should be a local fallback"),
            0
        );
        let mut counter = TypedInstrCounter::default();
        counter.visit_fn(caller);
        assert_eq!(counter.typed_calls, 1);
        assert_eq!(counter.guarded_method_calls, 0);
    }

    #[test]
    fn rejects_guarded_method_typed_call_access_without_getattr_target() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(fn):\n    return fn()\n",
        )
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

        let mut counter = TypedInstrCounter::default();
        counter.visit_instr(&guard);

        assert_eq!(counter.direct_call_guard_tests, 1);
        assert_eq!(
            counter.first_class,
            counter.loads + counter.direct_call_guard_tests
        );
        assert!(
            matches!(
                try_lower_typed_instr_to_codegen_legacy(guard),
                Ok(InstrCodegen::DirectFunctionIdGuardTest(_))
            ),
            "function-id direct-call guards should lower to the codegen guard representation"
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

        let mut counter = TypedInstrCounter::default();
        counter.visit_instr(&direct_call);

        assert_eq!(counter.direct_callable_calls, 1);
        assert_eq!(counter.loads, 2);
        assert_eq!(
            counter.first_class,
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

        let mut counter = TypedInstrCounter::default();
        counter.visit_instr(&direct_call);

        assert_eq!(counter.direct_method_calls, 1);
        assert_eq!(counter.loads, 2);
        assert_eq!(
            counter.first_class,
            counter.loads + counter.direct_method_calls
        );
        assert!(
            try_lower_typed_instr_to_codegen_legacy(direct_call).is_err(),
            "typed direct method calls should not silently lower through the legacy adapter"
        );
    }
}
