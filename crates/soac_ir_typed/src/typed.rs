use crate::emit_v3::MechanicalRegionEmission;
use crate::plan_v3::{
    ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemGuardKind, ExactListItemShape,
    IndexedGlobalAccessKind, IndexedGlobalFallbackKind, IndexedGlobalGuardKind, RegionPlan,
};
use crate::value_facts::ValueFacts;
#[allow(unused_imports)]
use soac_core::block_py;
#[allow(unused_imports)]
use soac_core::block_py::{
    BinOp, Block, BlockPyFunction, BlockPyModule, Call, CallArgKeyword, CallArgPositional,
    CallDirect, CalleeFunctionId, CellRef, ChildVisitable, ConstantExpr, Del, DelItem, GetAttr,
    GetItem, HasMeta, HasSemanticInstrId, IncrementCounter, Instr, InstrId, InstrWithConstantNone,
    Load, LocalLocation, MakeCell, MakeFunctionWithClosure, MapFunction, MapInstr, MapModule,
    Mappable, Meta, ModuleShape, NameLike, PrettyPrint, PrettyPrinter, ResolvedName,
    RuntimeFunctionId, RuntimeName, SetAttr, SetItem, Store, TryMapInstr, Tuple, UnaryOp, Visit,
    VisitMut, WithMeta, define_instr, define_ruff_instr,
};
#[allow(unused_imports)]
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
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
    PackedRest { start: usize },
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
pub enum TypedDirectCallableCallGuard {
    Function(TypedDirectFunctionCallGuard),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedCallAccessPlan {
    Generic,
    GuardedCallable {
        function_guards: Vec<TypedDirectFunctionCallGuard>,
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
    },
    DirectCallable {
        function_guard: TypedDirectFunctionCallGuard,
    },
    Method {
        method_name: String,
        method_guards: Vec<TypedDirectMethodCallGuard>,
    },
    RuntimeProtocolMethod {
        runtime_name: RuntimeName,
        method_name: String,
        method_guards: Vec<TypedDirectMethodCallGuard>,
    },
}

impl TypedCallEmissionPlan {
    pub fn target_function_ids(&self) -> Vec<RuntimeFunctionId> {
        match self {
            Self::Callable { function_guards } => function_guards
                .iter()
                .map(|guard| guard.function_id)
                .collect(),
            Self::DirectCallable { function_guard } => vec![function_guard.function_id],
            Self::Method { method_guards, .. } => method_guards
                .iter()
                .map(|guard| guard.function_id)
                .collect(),
            Self::RuntimeProtocolMethod { method_guards, .. } => method_guards
                .iter()
                .map(|guard| guard.function_id)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Callable { function_guards } => function_guards.is_empty(),
            Self::DirectCallable { .. } => false,
            Self::Method { method_guards, .. } => method_guards.is_empty(),
            Self::RuntimeProtocolMethod { method_guards, .. } => method_guards.is_empty(),
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

    pub fn from_typed_call(call: TypedCall<E>, guard: TypedDirectCallableCallGuard) -> Self {
        Self {
            _meta: call._meta,
            extra: call.extra,
            func: call.func,
            args: call.args,
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
}

impl<E: Instr> TypedGuardedCallableCall<E> {
    pub fn from_typed_call(
        call: TypedCall<E>,
        function_guards: Vec<TypedDirectFunctionCallGuard>,
    ) -> Self {
        Self {
            _meta: call._meta,
            extra: call.extra,
            func: call.func,
            args: call.args,
            keywords: call.keywords,
            function_guards,
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
            format_args!(", function_guards: {:?} }}", self.function_guards),
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
    ExactTypeVersion {
        function_id: RuntimeFunctionId,
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
    OptimizationPlanV3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypedIndexedFieldCounterSource {
    pub function_id: RuntimeFunctionId,
    pub instr_id: InstrId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedAttrAccessPlan {
    Generic,
    IndexedField {
        source: TypedIndexedFieldPlanSource,
        counter_source: Option<TypedIndexedFieldCounterSource>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypedExactListItemCounterSource {
    pub function_id: RuntimeFunctionId,
    pub instr_id: InstrId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExactListItemAccessPlan {
    pub source: TypedExactListItemPlanSource,
    pub instr_id: InstrId,
    pub counter_source: Option<TypedExactListItemCounterSource>,
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
    BorrowedLocal { location: LocalLocation },
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
    pub const PYOBJECT_IMMORTAL: Self = Self::PyObject {
        ownership: TypedPyObjectOwnershipPlan::Immortal,
    };
    pub const I32_BOOL01: Self = Self::I32Bool01;
    pub const I64_VALUE: Self = Self::I64;

    pub const fn pyobject_borrowed_local(location: LocalLocation) -> Self {
        Self::PyObject {
            ownership: TypedPyObjectOwnershipPlan::BorrowedLocal { location },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedConstructorInitPlanSource {
    InlinedConstructorEntry,
    InlinedConstructorEntryWithInlinedInitBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypedConstructorInitPlan {
    pub source: TypedConstructorInitPlanSource,
    pub init_function_id: RuntimeFunctionId,
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
    pub constructor_init: Option<TypedConstructorInitPlan>,
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

    pub fn exact_int_branch_plan_mut(&mut self) -> Option<&mut TypedExactIntBranchPlan> {
        self.exact_int_branch.as_mut()
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

    pub fn exact_int_return_plan_mut(&mut self) -> Option<&mut TypedExactIntReturnPlan> {
        self.exact_int_return.as_mut()
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

    pub fn constructor_init_plan(&self) -> Option<TypedConstructorInitPlan> {
        self.constructor_init
    }

    pub fn set_constructor_init_plan(&mut self, plan: TypedConstructorInitPlan) -> bool {
        if self.constructor_init == Some(plan) {
            return false;
        }
        self.constructor_init = Some(plan);
        true
    }

    pub fn clear_constructor_init_plan(&mut self) -> bool {
        self.constructor_init.take().is_some()
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

struct BlockPyToTyped;

impl MapInstr<InstrBlockPy, InstrTyped> for BlockPyToTyped {
    fn map_instr(&mut self, instr: InstrBlockPy) -> InstrTyped {
        match instr {
            InstrBlockPy::BinOp(op) => InstrTyped::BinOp(op.map_children(self)),
            InstrBlockPy::Tuple(op) => InstrTyped::Tuple(op.map_children(self)),
            InstrBlockPy::UnaryOp(op) => InstrTyped::UnaryOp(op.map_children(self)),
            InstrBlockPy::Call(op) => {
                InstrTyped::CallTyped(TypedCall::from_legacy(op.map_children(self)))
            }
            InstrBlockPy::GetAttr(op) => {
                InstrTyped::GetAttrTyped(TypedGetAttr::from_legacy(op.map_children(self)))
            }
            InstrBlockPy::SetAttr(op) => {
                InstrTyped::SetAttrTyped(TypedSetAttr::from_legacy(op.map_children(self)))
            }
            InstrBlockPy::GetItem(op) => InstrTyped::GetItem(op.map_children(self)),
            InstrBlockPy::SetItem(op) => InstrTyped::SetItem(op.map_children(self)),
            InstrBlockPy::DelItem(op) => InstrTyped::DelItem(op.map_children(self)),
            InstrBlockPy::Load(op) => InstrTyped::Load(op.map_children(self)),
            InstrBlockPy::Store(op) => InstrTyped::Store(op.map_children(self)),
            InstrBlockPy::Del(op) => InstrTyped::Del(op.map_children(self)),
            InstrBlockPy::MakeCell(op) => InstrTyped::MakeCell(op.map_children(self)),
            InstrBlockPy::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrBlockPy::CellRef(op) => InstrTyped::CellRef(op),
            InstrBlockPy::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.map_children(self))
            }
        }
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}

pub fn lower_blockpy_module_to_typed(
    module: BlockPyModule<BlockPyModuleShape>,
) -> BlockPyModule<TypedBlockPyModuleShape> {
    BlockPyToTyped.map_module(module)
}

pub fn lower_blockpy_function_to_typed(
    function: BlockPyFunction<BlockPyModuleShape>,
) -> BlockPyFunction<TypedBlockPyModuleShape> {
    BlockPyToTyped.map_fn(function)
}

#[derive(Debug, Clone)]
pub struct TypedBlockPyModuleShape;

impl ModuleShape for TypedBlockPyModuleShape {
    type Instr = InstrTyped;
    type ModuleConstant = ConstantExpr;
    type BlockExtra = TypedBlockExtra;
}
