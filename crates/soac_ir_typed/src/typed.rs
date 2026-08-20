use crate::emit_v3::MechanicalRegionEmission;
use crate::native_iterator::TypedNativeIteratorPipelinePlan;
use crate::plan_v3::{
    ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemGuardKind, ExactListItemShape,
    IndexedGlobalAccessKind, IndexedGlobalFallbackKind, IndexedGlobalGuardKind, RegionPlan,
};
use crate::value_facts::ValueFacts;
#[allow(unused_imports)]
use soac_core::block_py;
#[allow(unused_imports)]
use soac_core::block_py::{
    ApplyClassDecorator, ApplyFunctionDescriptor, BinOp, Block, BlockPyFunction, BlockPyModule,
    Call, CallArgKeyword, CallArgPositional, CallDirect, CalleeFunctionId, CellRef,
    CheckAnnotationFormat, ChildVisitable, CompleteFunctionDefinition, ComprehensionInsert,
    ConstantExpr, ConstructClass, ConstructTypeParameterScope, CreateTypeAlias,
    CreateTypeParameter, Del, DelItem, DiscardClassConstructionCaptures, DiscardClassDecorator,
    FrameNamespace, GetAttr, GetItem, HasMeta, HasSemanticInstrId, IncrementCounter, Instr,
    InstrId, InstrWithConstantNone, Load, LocalLocation, MakeCell, MakeFunctionWithClosure,
    MapFunction, MapInstr, MapModule, Mappable, Meta, ModuleShape, NameLike, NewAnnotationSet,
    PrepareClassDecorator, PrettyPrint, PrettyPrinter, RecordAnnotation, ResolvedName,
    RuntimeFunctionId, RuntimeName, SetAttr, SetFunctionTypeParameters, SetItem,
    SetTypeParameterDefault, SetupAnnotations, Store, SubscriptGeneric, TakeOperand, TryMapInstr,
    Tuple, UnaryOp, Visit, VisitMut, WithMeta, define_instr, define_ruff_instr,
};
use soac_core::block_py::{BuildCollection, CallArgumentOp, IteratorStep, PreparedCall};
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
    /// A source-bound request, not a receiver or argument proof. The active
    /// function supplies the actual immutable family capability at this slot.
    GuardedSealedMethod(Box<TypedSealedMethodAccessPlan>),
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
    pub frame_namespace: Option<FrameNamespace<E>>,
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
            frame_namespace: None,
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
            frame_namespace: op.frame_namespace,
            access: TypedCallAccessPlan::Generic,
        }
    }

    pub fn into_legacy(self) -> Call<E> {
        Call::new(self.func, self.args, self.keywords)
            .with_frame_namespace(self.frame_namespace)
            .with_extra(self.extra)
            .with_meta(self._meta)
    }
}

impl TypedCall<InstrTyped> {
    /// Preserve the same explicit input-ownership shape through planning and
    /// codegen. Ordinary loads and non-generic call plans are not selected.
    pub fn has_owned_operand_inputs(
        &self,
        layout: &block_py::StorageLayout,
    ) -> Result<bool, String> {
        if !matches!(&self.access, TypedCallAccessPlan::Generic) {
            return Ok(false);
        }
        block_py::call_has_owned_operand_inputs(
            self.func.as_ref(),
            &self.args,
            !self.keywords.is_empty(),
            self.frame_namespace.is_some(),
            layout,
            |input| matches!(input, InstrTyped::CallTyped(_)),
        )
    }
}

impl<E: Instr + std::fmt::Debug> std::fmt::Debug for TypedCall<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedCall")
            .field("func", &self.func)
            .field("args", &self.args)
            .field("keywords", &self.keywords)
            .field("frame_namespace", &self.frame_namespace)
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
        if let Some(FrameNamespace::Mapping(namespace)) = &self.frame_namespace {
            visitor.visit_instr(namespace);
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
        if let Some(FrameNamespace::Mapping(namespace)) = &mut self.frame_namespace {
            visitor.visit_instr_mut(namespace);
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
            frame_namespace: self
                .frame_namespace
                .map(|value| value.map_instr(|value| map.map_instr(value))),
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
            frame_namespace: self
                .frame_namespace
                .map(|value| value.try_map_instr(|value| map.try_map_instr(value)))
                .transpose()?,
        })
    }

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        TypedCall {
            _meta: self._meta,
            extra: self.extra,
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
            frame_namespace: self
                .frame_namespace
                .map(|value| value.map_instr(|value| map.map_instr(value))),
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(TypedCall {
            _meta: self._meta,
            extra: self.extra,
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
            frame_namespace: self
                .frame_namespace
                .map(|value| value.try_map_instr(|value| map.try_map_instr(value)))
                .transpose()?,
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
        assert!(
            call.frame_namespace.is_none(),
            "direct calls require an explicit class-frame transfer plan"
        );
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

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        TypedDirectCallableCall {
            _meta: self._meta,
            extra: self.extra,
            func: Box::new(map.map_instr(*self.func)),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.map_instr(|expr| map.map_instr(expr)))
                .collect(),
            guard: self.guard,
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(TypedDirectCallableCall {
            _meta: self._meta,
            extra: self.extra,
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
        assert!(
            call.frame_namespace.is_none(),
            "guarded calls require an explicit class-frame transfer plan"
        );
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
            frame_namespace: None,
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

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        TypedGuardedCallableCall {
            _meta: self._meta,
            extra: self.extra,
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

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(TypedGuardedCallableCall {
            _meta: self._meta,
            extra: self.extra,
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
        assert!(
            call.frame_namespace.is_none(),
            "guarded methods require an explicit class-frame transfer plan"
        );
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
            frame_namespace: None,
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

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        TypedGuardedMethodCall {
            _meta: self._meta,
            extra: self.extra,
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

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(TypedGuardedMethodCall {
            _meta: self._meta,
            extra: self.extra,
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

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        TypedDirectMethodCall {
            _meta: self._meta,
            extra: self.extra,
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

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(TypedDirectMethodCall {
            _meta: self._meta,
            extra: self.extra,
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
pub struct TypedLateBoundOwnerFieldPlan {
    pub counter_source: TypedIndexedFieldCounterSource,
    pub owner_type: crate::plan_v3::IndexedFieldOwnerType,
    pub attr_name: String,
    pub storage: crate::plan_v3::LateBoundOwnerFieldStorage,
    pub cell_index: u32,
}

/// An authenticated source-site request for an optional runtime capability.
/// It contains no predicted offset or receiver proof. The active function's
/// matching slot must hold a sealed construction witness before a raw load;
/// a missing slot or failed receiver/lookup guard uses ordinary getattr.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedSealedFieldAccessPlan {
    pub site: soac_contracts::AttributeSiteIdentity,
    pub receiver_class: soac_contracts::ClassReference,
    pub name: String,
    pub capability_slot: u32,
}

/// The method layout is independent of physical instance-field storage. A
/// request names one source attribute; only actual class adoption can fill its
/// runtime slot with a particular construction's family and dispatch position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedSealedMethodAccessPlan {
    pub site: soac_contracts::AttributeSiteIdentity,
    pub receiver_class: soac_contracts::ClassReference,
    pub name: String,
    pub capability_slot: u32,
}

/// Request an authenticated positional source-body call. The actual callable
/// and current entry remain guarded; the ordinary binder supplies defaults
/// and reports binding errors. This plan carries no argument or return types.
/// Methods include their receiver in the full bound body arity. Unsupported
/// shapes use the captured public entry without replaying evaluation/binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedSourceCallPlan {
    pub caller_source: soac_contracts::SourceIdentity,
    pub argument_count: usize,
    pub body_target: Option<TypedSourceBodyTarget>,
}

/// A source-selected native target, never unchecked callable authority. After
/// ordinary binding the actual activation's pinned body pointer must
/// equal this declared target before a fixed native call may use its actual
/// environment. An override or a different compilation keeps virtual dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedSourceBodyTarget {
    pub source: soac_contracts::SourceIdentity,
    pub function_id: RuntimeFunctionId,
    pub argument_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedAttrAccessPlan {
    Generic,
    GuardedSealedField(Box<TypedSealedFieldAccessPlan>),
    IndexedField {
        source: TypedIndexedFieldPlanSource,
        counter_source: Option<TypedIndexedFieldCounterSource>,
        guards: Vec<TypedIndexedFieldGuard>,
    },
    LateBoundOwnerField(TypedLateBoundOwnerFieldPlan),
    PolymorphicLateBoundOwnerFields(Vec<TypedLateBoundOwnerFieldPlan>),
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
    TakeOperand(TakeOperand<Self>),
    ComprehensionInsert(ComprehensionInsert<Self>),
    BuildCollection(BuildCollection<Self>),
    CallArgumentOp(CallArgumentOp<Self>),
    PreparedCall(PreparedCall<Self>),
    IteratorStep(IteratorStep<Self>),
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
    IncrementCounter(IncrementCounter),
    CellRef(CellRef),
    MakeFunctionWithClosure(MakeFunctionWithClosure<Self>),
    CompleteFunctionDefinition(CompleteFunctionDefinition<Self>),
    ApplyFunctionDescriptor(ApplyFunctionDescriptor<Self>),
    PrepareClassDecorator(PrepareClassDecorator<Self>),
    ApplyClassDecorator(ApplyClassDecorator<Self>),
    DiscardClassDecorator(DiscardClassDecorator<Self>),
    DiscardClassConstructionCaptures(DiscardClassConstructionCaptures<Self>),
    ConstructClass(ConstructClass<Self>),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedGeneratorInstancePlan {
    pub function_id: RuntimeFunctionId,
    pub kind: block_py::FunctionKind,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedBuiltinImplementationPlan {
    pub source: RuntimeName,
    pub function_id: RuntimeFunctionId,
    pub arg_plan: TypedDirectCallArgPlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedGeneratorResumePlan {
    pub function_id: RuntimeFunctionId,
    pub generator_origin: Option<InstrId>,
    pub candidate_origins: Vec<InstrId>,
}

/// A resolved operand for an opaque fused-iteration entry guard.
///
/// The distinction is intentional: entry guards must not run ordinary Python
/// lookup code before deciding whether to execute the untouched fallback.
/// Locals are already evaluated values, indexed globals use a non-raising
/// probe, and runtime names are compiler-owned module constants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedOpaqueFusedGuardOperand {
    Local(ResolvedName),
    IndexedGlobal { name: String, expected_index: u32 },
    RuntimeName(RuntimeName),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedOpaqueFusedGuardExpectation {
    FunctionIdentity {
        function_id: RuntimeFunctionId,
    },
    FunctionPositionalDefaultIdentity {
        function_id: RuntimeFunctionId,
        default_index: u32,
        expected_defaults_len: u32,
        expected: RuntimeName,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedOpaqueFusedEntryGuard {
    pub operand: TypedOpaqueFusedGuardOperand,
    pub expectation: TypedOpaqueFusedGuardExpectation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedOpaqueFusedResult {
    Count,
    Discard,
}

/// JIT-facing, fully resolved form of an opaque fused-iteration decision.
///
/// The plan is attached to the original root expression. Codegen emits the
/// guarded scalar fast path and clones that same expression, with this
/// sidecar cleared, for the cold fallback. Keeping the fallback in the typed
/// program avoids reconstructing or partially resuming generator state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedOpaqueFusedIterationPlan {
    pub source: InstrId,
    pub result: TypedOpaqueFusedResult,
    pub width_input: ResolvedName,
    pub minimum_width: i64,
    pub maximum_width: i64,
    pub entry_guards: Vec<TypedOpaqueFusedEntryGuard>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExactFloatExpressionPlan {
    pub source: InstrId,
    pub operations: Vec<crate::plan_v3::ExactFloatExpressionOperationPlan>,
    pub leaf_sources: Vec<InstrId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypedInstrExtra {
    pub result_facts: Option<ValueFacts>,
    pub demand: Option<TypedResultDemand>,
    pub planned_result: Option<TypedPlannedResult>,
    pub trusted_object_origin_candidates: Option<Vec<InstrId>>,
    pub trusted_generator_resume_function: Option<RuntimeFunctionId>,
    pub indexed_global_access: Option<TypedIndexedGlobalAccessPlan>,
    pub exact_list_item_access: Option<TypedExactListItemAccessPlan>,
    // Complete regions are optional sidecars, not inline storage paid by every
    // recursive expression. Inlining clones and remaps only selected regions.
    pub exact_int_branch: Option<Box<TypedExactIntBranchPlan>>,
    pub exact_int_return: Option<Box<TypedExactIntReturnPlan>>,
    pub constructor_init: Option<TypedConstructorInitPlan>,
    pub builtin_implementation: Option<TypedBuiltinImplementationPlan>,
    pub native_iterator_pipeline: Option<Box<TypedNativeIteratorPipelinePlan>>,
    pub resolved_descriptor_function_guards: Option<Vec<TypedDirectFunctionCallGuard>>,
    pub generator_instance: Option<TypedGeneratorInstancePlan>,
    pub generator_resume: Option<TypedGeneratorResumePlan>,
    pub opaque_fused_iteration: Option<TypedOpaqueFusedIterationPlan>,
    pub exact_float_expression: Option<TypedExactFloatExpressionPlan>,
    // This source/ABI sidecar is large and present only on selected calls.
    // Do not make every recursive typed expression pay its inline size.
    pub source_call: Option<Box<TypedSourceCallPlan>>,
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
    pub context: soac_core::block_py::BlockContext,
}

impl soac_core::block_py::HasBlockContext for TypedBlockExtra {
    fn block_context(&self) -> soac_core::block_py::BlockContext {
        self.context
    }

    fn set_block_context(&mut self, context: soac_core::block_py::BlockContext) {
        self.context = context;
    }
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

    pub fn trusted_object_origin_candidates(&self) -> Option<&[InstrId]> {
        self.trusted_object_origin_candidates.as_deref()
    }

    pub fn set_trusted_object_origin_candidates(
        &mut self,
        candidate_origins: Vec<InstrId>,
    ) -> bool {
        if self.trusted_object_origin_candidates.as_ref() == Some(&candidate_origins) {
            return false;
        }
        self.trusted_object_origin_candidates = Some(candidate_origins);
        true
    }

    pub fn clear_trusted_object_origin_candidates(&mut self) -> bool {
        self.trusted_object_origin_candidates.take().is_some()
    }

    pub fn trusted_generator_resume_function(&self) -> Option<RuntimeFunctionId> {
        self.trusted_generator_resume_function
    }

    pub fn set_trusted_generator_resume_function(
        &mut self,
        function_id: RuntimeFunctionId,
    ) -> bool {
        if self.trusted_generator_resume_function == Some(function_id) {
            return false;
        }
        self.trusted_generator_resume_function = Some(function_id);
        true
    }

    pub fn clear_trusted_generator_resume_function(&mut self) -> bool {
        self.trusted_generator_resume_function.take().is_some()
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
        self.exact_int_branch.as_deref()
    }

    pub fn exact_int_branch_plan_mut(&mut self) -> Option<&mut TypedExactIntBranchPlan> {
        self.exact_int_branch.as_deref_mut()
    }

    pub fn set_exact_int_branch_plan(&mut self, plan: TypedExactIntBranchPlan) -> bool {
        if self.exact_int_branch.as_deref() == Some(&plan) {
            return false;
        }
        self.exact_int_branch = Some(Box::new(plan));
        true
    }

    pub fn clear_exact_int_branch_plan(&mut self) -> bool {
        self.exact_int_branch.take().is_some()
    }

    pub fn exact_int_return_plan(&self) -> Option<&TypedExactIntReturnPlan> {
        self.exact_int_return.as_deref()
    }

    pub fn exact_int_return_plan_mut(&mut self) -> Option<&mut TypedExactIntReturnPlan> {
        self.exact_int_return.as_deref_mut()
    }

    pub fn set_exact_int_return_plan(&mut self, plan: TypedExactIntReturnPlan) -> bool {
        if self.exact_int_return.as_deref() == Some(&plan) {
            return false;
        }
        self.exact_int_return = Some(Box::new(plan));
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

    pub fn builtin_implementation_plan(&self) -> Option<&TypedBuiltinImplementationPlan> {
        self.builtin_implementation.as_ref()
    }

    pub fn set_builtin_implementation_plan(
        &mut self,
        plan: TypedBuiltinImplementationPlan,
    ) -> bool {
        if self.builtin_implementation.as_ref() == Some(&plan) {
            return false;
        }
        self.builtin_implementation = Some(plan);
        true
    }

    pub fn clear_builtin_implementation_plan(&mut self) -> bool {
        self.builtin_implementation.take().is_some()
    }

    pub fn native_iterator_pipeline_plan(&self) -> Option<&TypedNativeIteratorPipelinePlan> {
        self.native_iterator_pipeline.as_deref()
    }

    pub fn set_native_iterator_pipeline_plan(
        &mut self,
        plan: TypedNativeIteratorPipelinePlan,
    ) -> bool {
        if self.native_iterator_pipeline.as_deref() == Some(&plan) {
            return false;
        }
        self.native_iterator_pipeline = Some(Box::new(plan));
        true
    }

    pub fn clear_native_iterator_pipeline_plan(&mut self) -> bool {
        self.native_iterator_pipeline.take().is_some()
    }

    pub fn generator_instance_plan(&self) -> Option<&TypedGeneratorInstancePlan> {
        self.generator_instance.as_ref()
    }

    pub fn set_generator_instance_plan(&mut self, plan: TypedGeneratorInstancePlan) -> bool {
        if self.generator_instance.as_ref() == Some(&plan) {
            return false;
        }
        self.generator_instance = Some(plan);
        true
    }

    pub fn clear_generator_instance_plan(&mut self) -> bool {
        self.generator_instance.take().is_some()
    }

    pub fn generator_resume_plan(&self) -> Option<TypedGeneratorResumePlan> {
        self.generator_resume.clone()
    }

    pub fn set_generator_resume_plan(&mut self, plan: TypedGeneratorResumePlan) -> bool {
        if self.generator_resume.as_ref() == Some(&plan) {
            return false;
        }
        self.generator_resume = Some(plan);
        true
    }

    pub fn clear_generator_resume_plan(&mut self) -> bool {
        self.generator_resume.take().is_some()
    }

    pub fn opaque_fused_iteration_plan(&self) -> Option<&TypedOpaqueFusedIterationPlan> {
        self.opaque_fused_iteration.as_ref()
    }

    pub fn set_opaque_fused_iteration_plan(&mut self, plan: TypedOpaqueFusedIterationPlan) -> bool {
        if self.opaque_fused_iteration.as_ref() == Some(&plan) {
            return false;
        }
        self.opaque_fused_iteration = Some(plan);
        true
    }

    pub fn clear_opaque_fused_iteration_plan(&mut self) -> bool {
        self.opaque_fused_iteration.take().is_some()
    }

    pub fn exact_float_expression_plan(&self) -> Option<&TypedExactFloatExpressionPlan> {
        self.exact_float_expression.as_ref()
    }

    pub fn set_exact_float_expression_plan(&mut self, plan: TypedExactFloatExpressionPlan) -> bool {
        if self.exact_float_expression.as_ref() == Some(&plan) {
            return false;
        }
        self.exact_float_expression = Some(plan);
        true
    }

    pub fn clear_exact_float_expression_plan(&mut self) -> bool {
        self.exact_float_expression.take().is_some()
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
            Self::TakeOperand(op) => Some(op.extra()),
            Self::ComprehensionInsert(op) => Some(op.extra()),
            Self::BuildCollection(op) => Some(op.extra()),
            Self::CallArgumentOp(op) => Some(op.extra()),
            Self::PreparedCall(op) => Some(op.extra()),
            Self::IteratorStep(op) => Some(op.extra()),
            Self::MakeCell(op) => Some(op.extra()),
            Self::NewAnnotationSet(op) => Some(op.extra()),
            Self::SetupAnnotations(op) => Some(op.extra()),
            Self::ConstructTypeParameterScope(op) => Some(op.extra()),
            Self::SubscriptGeneric(op) => Some(op.extra()),
            Self::SetFunctionTypeParameters(op) => Some(op.extra()),
            Self::CreateTypeAlias(op) => Some(op.extra()),
            Self::CreateTypeParameter(op) => Some(op.extra()),
            Self::SetTypeParameterDefault(op) => Some(op.extra()),
            Self::CheckAnnotationFormat(op) => Some(op.extra()),
            Self::RecordAnnotation(op) => Some(op.extra()),
            Self::MakeFunctionWithClosure(op) => Some(op.extra()),
            Self::ConstructClass(op) => Some(op.extra()),
            Self::PrepareClassDecorator(op) => Some(op.extra()),
            Self::ApplyClassDecorator(op) => Some(op.extra()),
            Self::DiscardClassDecorator(op) => Some(op.extra()),
            Self::DiscardClassConstructionCaptures(op) => Some(op.extra()),
            Self::CompleteFunctionDefinition(op) => Some(op.extra()),
            Self::ApplyFunctionDescriptor(op) => Some(op.extra()),
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
            Self::TakeOperand(op) => Some(op.extra_mut()),
            Self::ComprehensionInsert(op) => Some(op.extra_mut()),
            Self::BuildCollection(op) => Some(op.extra_mut()),
            Self::CallArgumentOp(op) => Some(op.extra_mut()),
            Self::PreparedCall(op) => Some(op.extra_mut()),
            Self::IteratorStep(op) => Some(op.extra_mut()),
            Self::MakeCell(op) => Some(op.extra_mut()),
            Self::NewAnnotationSet(op) => Some(op.extra_mut()),
            Self::SetupAnnotations(op) => Some(op.extra_mut()),
            Self::ConstructTypeParameterScope(op) => Some(op.extra_mut()),
            Self::SubscriptGeneric(op) => Some(op.extra_mut()),
            Self::SetFunctionTypeParameters(op) => Some(op.extra_mut()),
            Self::CreateTypeAlias(op) => Some(op.extra_mut()),
            Self::CreateTypeParameter(op) => Some(op.extra_mut()),
            Self::SetTypeParameterDefault(op) => Some(op.extra_mut()),
            Self::CheckAnnotationFormat(op) => Some(op.extra_mut()),
            Self::RecordAnnotation(op) => Some(op.extra_mut()),
            Self::MakeFunctionWithClosure(op) => Some(op.extra_mut()),
            Self::ConstructClass(op) => Some(op.extra_mut()),
            Self::PrepareClassDecorator(op) => Some(op.extra_mut()),
            Self::ApplyClassDecorator(op) => Some(op.extra_mut()),
            Self::DiscardClassDecorator(op) => Some(op.extra_mut()),
            Self::DiscardClassConstructionCaptures(op) => Some(op.extra_mut()),
            Self::CompleteFunctionDefinition(op) => Some(op.extra_mut()),
            Self::ApplyFunctionDescriptor(op) => Some(op.extra_mut()),
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

    pub fn builtin_implementation_plan(&self) -> Option<&TypedBuiltinImplementationPlan> {
        self.typed_extra()
            .and_then(TypedInstrExtra::builtin_implementation_plan)
    }

    pub fn native_iterator_pipeline_plan(&self) -> Option<&TypedNativeIteratorPipelinePlan> {
        self.typed_extra()
            .and_then(TypedInstrExtra::native_iterator_pipeline_plan)
    }

    pub fn generator_instance_plan(&self) -> Option<&TypedGeneratorInstancePlan> {
        self.typed_extra()
            .and_then(TypedInstrExtra::generator_instance_plan)
    }

    pub fn generator_resume_plan(&self) -> Option<TypedGeneratorResumePlan> {
        self.typed_extra()
            .and_then(TypedInstrExtra::generator_resume_plan)
    }

    pub fn opaque_fused_iteration_plan(&self) -> Option<&TypedOpaqueFusedIterationPlan> {
        self.typed_extra()
            .and_then(TypedInstrExtra::opaque_fused_iteration_plan)
    }

    pub fn exact_float_expression_plan(&self) -> Option<&TypedExactFloatExpressionPlan> {
        self.typed_extra()
            .and_then(TypedInstrExtra::exact_float_expression_plan)
    }

    pub fn guard_miss_deopt_enabled(&self) -> bool {
        self.typed_extra()
            .is_some_and(TypedInstrExtra::guard_miss_deopt_enabled)
    }
}

impl soac_core::block_py::TakeOperandInstruction for InstrTyped {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
        match self {
            Self::TakeOperand(op) => Some(op),
            _ => None,
        }
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
            InstrBlockPy::NewAnnotationSet(op) => {
                InstrTyped::NewAnnotationSet(op.map_children(self))
            }
            InstrBlockPy::SetupAnnotations(op) => {
                InstrTyped::SetupAnnotations(op.map_children(self))
            }
            InstrBlockPy::ConstructTypeParameterScope(op) => {
                InstrTyped::ConstructTypeParameterScope(op.map_children(self))
            }
            InstrBlockPy::SubscriptGeneric(op) => {
                InstrTyped::SubscriptGeneric(op.map_children(self))
            }
            InstrBlockPy::SetFunctionTypeParameters(op) => {
                InstrTyped::SetFunctionTypeParameters(op.map_children(self))
            }
            InstrBlockPy::CreateTypeAlias(op) => InstrTyped::CreateTypeAlias(op.map_children(self)),
            InstrBlockPy::CreateTypeParameter(op) => {
                InstrTyped::CreateTypeParameter(op.map_children(self))
            }
            InstrBlockPy::SetTypeParameterDefault(op) => {
                InstrTyped::SetTypeParameterDefault(op.map_children(self))
            }
            InstrBlockPy::CheckAnnotationFormat(op) => {
                InstrTyped::CheckAnnotationFormat(op.map_children(self))
            }
            InstrBlockPy::RecordAnnotation(op) => {
                InstrTyped::RecordAnnotation(op.map_children(self))
            }
            InstrBlockPy::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrBlockPy::CellRef(op) => InstrTyped::CellRef(op),
            InstrBlockPy::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.map_children(self))
            }
            InstrBlockPy::ConstructClass(op) => InstrTyped::ConstructClass(op.map_children(self)),
            InstrBlockPy::PrepareClassDecorator(op) => {
                InstrTyped::PrepareClassDecorator(op.map_children(self))
            }
            InstrBlockPy::ApplyClassDecorator(op) => {
                InstrTyped::ApplyClassDecorator(op.map_children(self))
            }
            InstrBlockPy::DiscardClassDecorator(op) => {
                InstrTyped::DiscardClassDecorator(op.map_children(self))
            }
            InstrBlockPy::TakeOperand(op) => InstrTyped::TakeOperand(op.map_children(self)),
            InstrBlockPy::ComprehensionInsert(op) => {
                InstrTyped::ComprehensionInsert(op.map_children(self))
            }
            InstrBlockPy::BuildCollection(op) => InstrTyped::BuildCollection(op.map_children(self)),
            InstrBlockPy::CallArgumentOp(op) => InstrTyped::CallArgumentOp(op.map_children(self)),
            InstrBlockPy::PreparedCall(op) => InstrTyped::PreparedCall(op.map_children(self)),
            InstrBlockPy::IteratorStep(op) => InstrTyped::IteratorStep(op.map_children(self)),
            InstrBlockPy::DiscardClassConstructionCaptures(op) => {
                InstrTyped::DiscardClassConstructionCaptures(op.map_children(self))
            }
            InstrBlockPy::CompleteFunctionDefinition(op) => {
                InstrTyped::CompleteFunctionDefinition(op.map_children(self))
            }
            InstrBlockPy::ApplyFunctionDescriptor(op) => {
                InstrTyped::ApplyFunctionDescriptor(op.map_children(self))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_fused_iteration_sidecar_set_replace_and_clear_are_structural() {
        let width_input = ResolvedName {
            id: "n".into(),
            location: block_py::NameLocation::local(0),
        };
        let plan = TypedOpaqueFusedIterationPlan {
            source: InstrId::new(17),
            result: TypedOpaqueFusedResult::Count,
            width_input,
            minimum_width: 0,
            maximum_width: 8,
            entry_guards: vec![TypedOpaqueFusedEntryGuard {
                operand: TypedOpaqueFusedGuardOperand::IndexedGlobal {
                    name: "permutations".to_string(),
                    expected_index: 3,
                },
                expectation: TypedOpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity {
                    function_id: RuntimeFunctionId::from_raw_parts(1, 2),
                    default_index: 0,
                    expected_defaults_len: 1,
                    expected: RuntimeName::None,
                },
            }],
        };
        let mut extra = TypedInstrExtra::default();

        assert!(extra.set_opaque_fused_iteration_plan(plan.clone()));
        assert!(!extra.set_opaque_fused_iteration_plan(plan.clone()));
        assert_eq!(extra.opaque_fused_iteration_plan(), Some(&plan));

        let mut replacement = plan;
        replacement.maximum_width = 7;
        assert!(extra.set_opaque_fused_iteration_plan(replacement.clone()));
        assert_eq!(extra.opaque_fused_iteration_plan(), Some(&replacement));
        assert!(extra.clear_opaque_fused_iteration_plan());
        assert!(!extra.clear_opaque_fused_iteration_plan());
        assert_eq!(extra.opaque_fused_iteration_plan(), None);
    }
}
