pub use self::meta::{HasMeta, InstrId, Meta, WithMeta};
use self::operation_macro::define_operation;
pub use self::param_specs::{Param, ParamDefaultSource, ParamKind, ParamSpec};
pub(crate) use self::scope::{
    build_storage_layout_from_capture_names, compute_make_function_capture_bindings_from_scope,
    compute_storage_layout_from_scope, derive_effective_binding_for_name, ScopeExprNode,
};
pub use self::scope::{
    BindingKind, BindingPurpose, BindingTarget, CallableScopeInfo, CallableScopeKind,
    CellBindingKind, CellCaptureBinding, ClassBodyFallback, ClosureInit, ClosureSlot,
    EffectiveBinding, StorageLayout,
};
use crate::py_expr;
pub use operation::{
    Await, BinOp, BinOpKind, CalleeFunctionId, Call, CallDirect, CellRef, CellRefForName, Del,
    DelItem, GetAttr, GetItem, Load, MakeCell, MakeFunction, SetAttr, SetItem, Store, UnaryOp,
    UnaryOpKind, Yield, YieldFrom,
};
pub use ruff_python_ast::Expr;
use ruff_python_ast::{self as ast};
use soac_macros::enum_broadcast;
use std::fmt;

pub(crate) mod cfg;
mod map;
mod meta;
mod name_gen;
pub mod operation;
mod operation_macro;
pub(crate) mod param_specs;
pub mod pretty;
pub(crate) mod scope;
pub(crate) mod validate;
mod visit;
pub use crate::passes::{
    InstrLow, InstrWithAwaitAndYield, InstrWithYield, InstrResolved,
};
#[allow(unused_imports)]
pub(crate) use map::{
    MapBlock, MapFunction, MapModule, MapTerm, TryMapBlock, TryMapFunction, TryMapModule,
    TryMapTerm,
};
pub use map::{MapInstr, TryMapInstr};
pub use name_gen::{BlockLabel, FunctionId, FunctionNameGen, ModuleNameGen};
pub(crate) use validate::validate_module;
#[allow(unused_imports)]
pub(crate) use visit::{
    instr_any, walk_block, walk_block_mut, walk_expr, walk_expr_mut, walk_fn, walk_fn_mut,
    walk_module, walk_module_mut, walk_stmt, walk_stmt_mut, walk_term, walk_term_mut,
};
pub use visit::{Visit, VisitMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CounterId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CounterScope {
    This,
    Function,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CounterSite {
    BlockEntry {
        function_id: FunctionId,
        block_label: BlockLabel,
    },
    Runtime {
        function_id: Option<FunctionId>,
        instr_id: Option<InstrId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CounterDef {
    pub id: CounterId,
    pub scope: CounterScope,
    pub kind: String,
    pub site: CounterSite,
}
fn is_internal_symbol(name: &str) -> bool {
    name.starts_with("_dp_") || name == "__soac__"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalLocation(pub u32);

impl LocalLocation {
    pub fn slot(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalSlot(pub u32);

impl GlobalSlot {
    pub fn slot(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellLocation {
    Owned(u32),
    Closure(u32),
    CapturedSource(u32),
}

impl CellLocation {
    pub fn slot(self) -> u32 {
        match self {
            Self::Owned(slot) | Self::Closure(slot) | Self::CapturedSource(slot) => slot,
        }
    }

    pub fn is_owned(self) -> bool {
        matches!(self, Self::Owned(_))
    }

    pub fn is_closure(self) -> bool {
        matches!(self, Self::Closure(_))
    }

    pub fn is_captured_source(self) -> bool {
        matches!(self, Self::CapturedSource(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameLocation {
    Local(LocalLocation),
    Global(GlobalSlot),
    RuntimeName,
    Cell(CellLocation),
    Constant(u32),
}

impl NameLocation {
    pub fn local(slot: u32) -> Self {
        Self::Local(LocalLocation(slot))
    }

    pub fn global(slot: u32) -> Self {
        Self::Global(GlobalSlot(slot))
    }

    pub fn runtime_name() -> Self {
        Self::RuntimeName
    }

    pub fn owned_cell(slot: u32) -> Self {
        Self::Cell(CellLocation::Owned(slot))
    }

    pub fn closure_cell(slot: u32) -> Self {
        Self::Cell(CellLocation::Closure(slot))
    }

    pub fn captured_source_cell(slot: u32) -> Self {
        Self::Cell(CellLocation::CapturedSource(slot))
    }

    pub fn constant(index: u32) -> Self {
        Self::Constant(index)
    }

    pub fn as_local(self) -> Option<LocalLocation> {
        match self {
            Self::Local(location) => Some(location),
            Self::Global(_) | Self::RuntimeName | Self::Cell(_) | Self::Constant(_) => None,
        }
    }

    pub fn as_cell(self) -> Option<CellLocation> {
        match self {
            Self::Cell(location) => Some(location),
            Self::Local(_) | Self::Global(_) | Self::RuntimeName | Self::Constant(_) => None,
        }
    }

    pub fn as_constant(self) -> Option<u32> {
        match self {
            Self::Constant(index) => Some(index),
            Self::Local(_) | Self::Global(_) | Self::RuntimeName | Self::Cell(_) => None,
        }
    }

    pub fn as_global(self) -> Option<GlobalSlot> {
        match self {
            Self::Global(slot) => Some(slot),
            Self::Local(_) | Self::RuntimeName | Self::Cell(_) | Self::Constant(_) => None,
        }
    }

    pub fn is_global(self) -> bool {
        matches!(self, Self::Global(_))
    }

    pub fn is_runtime_name(self) -> bool {
        matches!(self, Self::RuntimeName)
    }

    pub fn pretty_id(self, unresolved_name: &str) -> String {
        match self {
            Self::Local(location) => format!("{location:?}"),
            Self::Global(slot) => format!("{unresolved_name}@g{}", slot.slot()),
            Self::RuntimeName => unresolved_name.to_string(),
            Self::Cell(location) => format!("{location:?}"),
            Self::Constant(index) => format!("constant slot {index}"),
        }
    }
}

pub trait BlockPyNameLike: Clone + fmt::Debug {
    fn id_str(&self) -> &str;
    fn pretty_id(&self) -> String {
        self.id_str().to_string()
    }
    fn is_runtime_name(&self) -> bool {
        false
    }
    fn is_runtime_symbol(&self, name: &str) -> bool {
        self.is_runtime_name() && self.id_str() == name
    }
}

#[derive(Debug, Clone)]
pub struct RuffExpr(pub ast::Expr);

impl From<ast::Expr> for RuffExpr {
    fn from(value: ast::Expr) -> Self {
        Self(value)
    }
}

impl From<RuffExpr> for ast::Expr {
    fn from(value: RuffExpr) -> Self {
        value.0
    }
}

pub trait ChildVisitable<E: Instr>: Clone + fmt::Debug + Sized {
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized;

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized;
}

pub trait Mappable<E>: Sized
where
    E: Instr,
{
    type Mapped<T: Instr>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>;

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>;

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        self.map_children(map)
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        self.try_map_children(map)
    }

}

pub trait Instr: Clone + fmt::Debug + Sized {
    type Name: BlockPyNameLike;
}

impl BlockPyNameLike for ast::ExprName {
    fn id_str(&self) -> &str {
        self.id.as_str()
    }
}

impl ChildVisitable<Expr> for Expr {
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<Expr> + ?Sized,
    {
        struct DirectChildVisitor<'a, V: ?Sized>(&'a mut V);

        impl<V> crate::block_py::VisitMut<Expr> for DirectChildVisitor<'_, V>
        where
            V: crate::block_py::Visit<Expr> + ?Sized,
        {
            fn visit_instr_mut(&mut self, expr: &mut Expr) {
                self.0.visit_instr(expr);
            }
        }

        let mut cloned = self.clone();
        cloned.visit_children_mut(&mut DirectChildVisitor(visitor));
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<Expr> + ?Sized,
    {
        struct DirectChildTransformer<'a, V: ?Sized>(&'a mut V);

        impl<V> crate::transformer::Transformer for DirectChildTransformer<'_, V>
        where
            V: crate::block_py::VisitMut<Expr> + ?Sized,
        {
            fn visit_expr(&mut self, expr: &mut Expr) {
                self.0.visit_instr_mut(expr);
            }
        }

        let mut transformer = DirectChildTransformer(visitor);
        crate::transformer::walk_expr(&mut transformer, self);
    }
}

impl Mappable<Expr> for Expr {
    type Mapped<T: Instr> = T;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<Expr, T>,
    {
        map.map_instr(self)
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<Expr, T, Error>,
    {
        map.try_map_instr(self)
    }

}

impl Instr for Expr {
    type Name = ast::ExprName;
}

impl Instr for RuffExpr {
    type Name = ast::ExprName;
}

#[derive(Clone)]
pub enum UnresolvedName {
    SourceName(ast::name::Name),
    RuntimeName(ast::name::Name),
}

impl fmt::Debug for UnresolvedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pretty_id())
    }
}

impl BlockPyNameLike for UnresolvedName {
    fn id_str(&self) -> &str {
        match self {
            Self::SourceName(name) | Self::RuntimeName(name) => name.as_str(),
        }
    }

    fn is_runtime_name(&self) -> bool {
        matches!(self, Self::RuntimeName(_))
    }
}

impl From<ast::ExprName> for UnresolvedName {
    fn from(value: ast::ExprName) -> Self {
        Self::SourceName(value.id)
    }
}

impl From<ast::name::Name> for UnresolvedName {
    fn from(value: ast::name::Name) -> Self {
        Self::SourceName(value)
    }
}

impl UnresolvedName {
    pub fn name(self) -> ast::name::Name {
        match self {
            Self::SourceName(name) | Self::RuntimeName(name) => name,
        }
    }
}

impl ChildVisitable<RuffExpr> for RuffExpr {
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<RuffExpr> + ?Sized,
    {
        struct RuffChildVisitor<'a, V: ?Sized>(&'a mut V);

        impl<V> crate::block_py::Visit<Expr> for RuffChildVisitor<'_, V>
        where
            V: crate::block_py::Visit<RuffExpr> + ?Sized,
        {
            fn visit_instr(&mut self, expr: &Expr) {
                self.0.visit_instr(&RuffExpr(expr.clone()));
            }
        }

        self.0.visit_children(&mut RuffChildVisitor(visitor));
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<RuffExpr> + ?Sized,
    {
        struct RuffChildVisitor<'a, V: ?Sized>(&'a mut V);

        impl<V> crate::block_py::VisitMut<Expr> for RuffChildVisitor<'_, V>
        where
            V: crate::block_py::VisitMut<RuffExpr> + ?Sized,
        {
            fn visit_instr_mut(&mut self, expr: &mut Expr) {
                let mut wrapped = RuffExpr(expr.clone());
                self.0.visit_instr_mut(&mut wrapped);
                *expr = wrapped.0;
            }
        }

        self.0.visit_children_mut(&mut RuffChildVisitor(visitor));
    }
}

impl Mappable<RuffExpr> for RuffExpr {
    type Mapped<T: Instr> = T;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<RuffExpr, T>,
    {
        map.map_instr(self)
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<RuffExpr, T, Error>,
    {
        map.try_map_instr(self)
    }

}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ResolvedName {
    pub id: ruff_python_ast::name::Name,
    pub location: NameLocation,
}

impl fmt::Debug for ResolvedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pretty_id())
    }
}

impl ResolvedName {
    pub fn with_location(mut self, location: NameLocation) -> Self {
        self.location = location;
        self
    }

    pub fn local_location(&self) -> Option<LocalLocation> {
        self.location.as_local()
    }

    pub fn cell_location(&self) -> Option<CellLocation> {
        self.location.as_cell()
    }

    pub fn resolved_pretty_id(&self) -> String {
        self.location.pretty_id(self.id.as_str())
    }

    pub fn is_runtime_name(&self) -> bool {
        self.location.is_runtime_name()
    }
}

impl BlockPyNameLike for ResolvedName {
    fn id_str(&self) -> &str {
        self.id.as_str()
    }

    fn pretty_id(&self) -> String {
        self.resolved_pretty_id()
    }

    fn is_runtime_name(&self) -> bool {
        self.location.is_runtime_name()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FunctionKind {
    Function,
    Coroutine,
    Generator,
    AsyncGenerator,
}

#[derive(Debug, Clone)]
pub struct Block<S, T: Instr = S> {
    pub label: BlockLabel,
    pub body: Vec<S>,
    pub term: BlockTerm<T>,
    pub params: Vec<BlockParam>,
    pub exc_edge: Option<BlockEdge>,
}

impl<S, T: Instr> Block<S, T> {
    pub fn label_str(&self) -> String {
        self.label.to_string()
    }
}

impl<S: NormalizedInstr, T: Instr> Block<S, T> {
    pub fn new(
        label: BlockLabel,
        body: Vec<S>,
        term: BlockTerm<T>,
        params: Vec<BlockParam>,
        exc_edge: Option<BlockEdge>,
    ) -> Self {
        let block = Self {
            label,
            body,
            term,
            params,
            exc_edge,
        };
        assert_blockpy_block_normalized(&block);
        block
    }
}

impl<S: NormalizedInstr, T: Instr> Block<S, T>
where
    BlockTerm<T>: BlockPyFallthroughTerm,
{
    pub fn from_builder(
        label: BlockLabel,
        builder: BlockBuilder<S, BlockTerm<T>>,
        params: Vec<BlockParam>,
        exc_edge: Option<BlockEdge>,
        fallthrough_target: Option<BlockLabel>,
    ) -> Self {
        Self::new(
            label,
            builder.body,
            builder.term.unwrap_or_else(|| match fallthrough_target {
                Some(target) => BlockTerm::<T>::jump_term(target),
                None => BlockTerm::<T>::implicit_function_return(),
            }),
            params,
            exc_edge,
        )
    }
}

impl<S, T: Instr> Block<S, T> {
    pub fn ensure_param(&mut self, name: impl Into<String>, role: BlockParamRole) {
        let name = name.into();
        if self.params.iter().any(|param| param.name == name) {
            return;
        }
        self.params.push(BlockParam { name, role });
    }

    pub fn set_exception_param(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.params
            .retain(|param| param.role != BlockParamRole::Exception || param.name == name);
        if let Some(param) = self.params.iter_mut().find(|param| param.name == name) {
            param.role = BlockParamRole::Exception;
            return;
        }
        self.params.push(BlockParam {
            name,
            role: BlockParamRole::Exception,
        });
    }

    pub fn exception_param(&self) -> Option<&str> {
        self.params
            .iter()
            .find(|param| param.role == BlockParamRole::Exception)
            .map(|param| param.name.as_str())
    }

    pub fn param_names(&self) -> impl Iterator<Item = &str> {
        self.params.iter().map(|param| param.name.as_str())
    }

    pub fn param_name_vec(&self) -> Vec<String> {
        self.param_names().map(ToString::to_string).collect()
    }

    pub fn bb_params(&self) -> impl Iterator<Item = &BlockParam> {
        [
            BlockParamRole::Exception,
            BlockParamRole::AbruptKind,
            BlockParamRole::AbruptPayload,
        ]
        .into_iter()
        .flat_map(|role| self.params.iter().filter(move |param| param.role == role))
    }

    pub fn bb_param_names(&self) -> impl Iterator<Item = &str> {
        self.bb_params().map(|param| param.name.as_str())
    }

    pub fn replace_fallthrough_target(&mut self, target: BlockLabel) -> bool {
        self.term.replace_target(BlockLabel::fallthrough(), target)
    }
}

#[derive(Debug, Clone, Default)]
pub struct BlockPyModule<P: BlockPyPass, S = <P as BlockPyPass>::Expr> {
    pub module_name_gen: ModuleNameGen,
    pub global_names: Vec<String>,
    pub callable_defs: Vec<BlockPyFunction<P, S>>,
    pub module_constants: Vec<InstrLow<ResolvedName>>,
    pub counter_defs: Vec<CounterDef>,
}

impl<P: BlockPyPass, S> BlockPyModule<P, S> {
    pub fn map_callable_defs<Q: BlockPyPass, T>(
        self,
        mut f: impl FnMut(BlockPyFunction<P, S>) -> BlockPyFunction<Q, T>,
    ) -> BlockPyModule<Q, T> {
        debug_assert!(
            self.module_constants.is_empty(),
            "map_callable_defs does not preserve module constants"
        );
        debug_assert!(
            self.counter_defs.is_empty(),
            "map_callable_defs does not preserve counter defs"
        );
        BlockPyModule {
            module_name_gen: self.module_name_gen,
            global_names: self.global_names,
            callable_defs: self.callable_defs.into_iter().map(&mut f).collect(),
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        }
    }
}

#[derive(Clone, derive_more::From)]
#[enum_broadcast(HasMeta, WithMeta, ChildVisitable, Mappable, Debug)]
pub enum CodegenBlockPyExpr {
    BinOp(BinOp<Self>),
    UnaryOp(UnaryOp<Self>),
    CalleeFunctionId(CalleeFunctionId<Self>),
    Call(Call<Self>),
    CallDirect(CallDirect<Self>),
    GetAttr(GetAttr<Self>),
    SetAttr(SetAttr<Self>),
    GetItem(GetItem<Self>),
    SetItem(SetItem<Self>),
    DelItem(DelItem<Self>),
    Load(Load<Self>),
    Store(Store<Self>),
    Del(Del<Self>),
    MakeCell(MakeCell<Self>),
    IncrementCounter(IncrementCounter),
    CellRef(CellRef),
    MakeFunction(MakeFunction<Self>),
}

define_operation! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}

#[derive(Clone, derive_more::From)]
pub enum BlockPyLiteral {
    StringLiteral(CoreStringLiteral),
    BytesLiteral(CoreBytesLiteral),
    NumberLiteral(CoreNumberLiteral),
}

impl fmt::Debug for BlockPyLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StringLiteral(value) => value.fmt(f),
            Self::BytesLiteral(value) => value.fmt(f),
            Self::NumberLiteral(value) => value.fmt(f),
        }
    }
}

define_operation! {
    pub struct LiteralValue {
        literal: BlockPyLiteral,
    }
}

impl LiteralValue {
    pub fn as_literal(&self) -> &BlockPyLiteral {
        &self.literal
    }

    pub fn into_literal(self) -> BlockPyLiteral {
        self.literal
    }
}

pub(crate) fn literal_value(literal: impl Into<BlockPyLiteral>, meta: Meta) -> LiteralValue {
    LiteralValue::new(literal.into()).with_meta(meta)
}

pub(crate) fn literal_expr<E>(literal: impl Into<BlockPyLiteral>, meta: Meta) -> E
where
    E: From<LiteralValue>,
{
    E::from(literal_value(literal, meta))
}

#[derive(Clone)]
pub struct CoreStringLiteral {
    pub value: String,
}

impl fmt::Debug for CoreStringLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

#[derive(Clone)]
pub struct CoreBytesLiteral {
    pub value: Vec<u8>,
}

impl fmt::Debug for CoreBytesLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

#[derive(Clone)]
pub struct CoreNumberLiteral {
    pub value: CoreNumberLiteralValue,
}

impl fmt::Debug for CoreNumberLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

#[derive(Clone)]
pub enum CoreNumberLiteralValue {
    Int(ast::Int),
    Float(f64),
}

impl fmt::Debug for CoreNumberLiteralValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value:?}"),
        }
    }
}

impl Instr for InstrWithAwaitAndYield {
    type Name = UnresolvedName;
}

impl Instr for InstrWithYield {
    type Name = UnresolvedName;
}

impl<N: BlockPyNameLike> Instr for InstrLow<N> {
    type Name = N;
}

impl Instr for CodegenBlockPyExpr {
    type Name = ResolvedName;
}

pub(crate) fn core_call_expr_with_meta<E>(
    func: E,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<CallArgPositional<E>>,
    keywords: Vec<CallArgKeyword<E>>,
) -> E
where
    E: Instr + From<Call<E>>,
{
    Call::new(func, args, keywords)
        .with_meta(Meta::new(node_index, range))
        .into()
}

pub(crate) fn core_runtime_name_expr_with_meta<E>(
    name: &str,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> E
where
    E: Instr<Name = UnresolvedName> + From<Load<E>>,
{
    Load::new(runtime_symbol(name))
        .with_meta(Meta::new(node_index, range))
        .into()
}

pub(crate) fn core_runtime_named_call_expr_with_meta<E>(
    func_name: &str,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<CallArgPositional<E>>,
    keywords: Vec<CallArgKeyword<E>>,
) -> E
where
    E: Instr<Name = UnresolvedName> + From<Call<E>> + From<Load<E>>,
{
    let func = core_runtime_name_expr_with_meta(func_name, node_index.clone(), range);
    core_call_expr_with_meta(func, node_index, range, args, keywords)
}

pub(crate) fn core_runtime_positional_call_expr_with_meta<E>(
    func_name: &str,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<E>,
) -> E
where
    E: Instr<Name = UnresolvedName> + From<Call<E>> + From<Load<E>>,
{
    core_runtime_named_call_expr_with_meta(
        func_name,
        node_index,
        range,
        args.into_iter()
            .map(CallArgPositional::Positional)
            .collect(),
        Vec::new(),
    )
}

pub(crate) fn runtime_symbol(name: &str) -> UnresolvedName {
    UnresolvedName::RuntimeName(name.into())
}

#[derive(Debug, Clone)]
pub enum CallArgPositional<E> {
    Positional(E),
    Starred(E),
}

impl<E> CallArgPositional<E> {
    pub fn expr(&self) -> &E {
        match self {
            Self::Positional(expr) | Self::Starred(expr) => expr,
        }
    }

    pub fn expr_mut(&mut self) -> &mut E {
        match self {
            Self::Positional(expr) | Self::Starred(expr) => expr,
        }
    }

    pub fn map_instr<T>(self, f: impl FnOnce(E) -> T) -> CallArgPositional<T> {
        match self {
            Self::Positional(expr) => CallArgPositional::Positional(f(expr)),
            Self::Starred(expr) => CallArgPositional::Starred(f(expr)),
        }
    }

    pub fn try_map_instr<T, Error>(
        self,
        f: impl FnOnce(E) -> Result<T, Error>,
    ) -> Result<CallArgPositional<T>, Error> {
        match self {
            Self::Positional(expr) => f(expr).map(CallArgPositional::Positional),
            Self::Starred(expr) => f(expr).map(CallArgPositional::Starred),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CallArgKeyword<E> {
    Named { arg: ast::Identifier, value: E },
    Starred(E),
}

impl<E> CallArgKeyword<E> {
    pub fn expr(&self) -> &E {
        match self {
            Self::Named { value, .. } | Self::Starred(value) => value,
        }
    }

    pub fn expr_mut(&mut self) -> &mut E {
        match self {
            Self::Named { value, .. } | Self::Starred(value) => value,
        }
    }

    pub fn map_instr<T>(self, f: impl FnOnce(E) -> T) -> CallArgKeyword<T> {
        match self {
            Self::Named { arg, value } => CallArgKeyword::Named {
                arg,
                value: f(value),
            },
            Self::Starred(value) => CallArgKeyword::Starred(f(value)),
        }
    }

    pub fn try_map_instr<T, Error>(
        self,
        f: impl FnOnce(E) -> Result<T, Error>,
    ) -> Result<CallArgKeyword<T>, Error> {
        match self {
            Self::Named { arg, value } => {
                f(value).map(|value| CallArgKeyword::Named { arg, value })
            }
            Self::Starred(value) => f(value).map(CallArgKeyword::Starred),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FunctionName {
    pub bind_name: String,
    pub fn_name: String,
    pub display_name: String,
    pub qualname: String,
}

impl FunctionName {
    pub fn new(
        bind_name: impl Into<String>,
        fn_name: impl Into<String>,
        display_name: impl Into<String>,
        qualname: impl Into<String>,
    ) -> Self {
        Self {
            bind_name: bind_name.into(),
            fn_name: fn_name.into(),
            display_name: display_name.into(),
            qualname: qualname.into(),
        }
    }
}

#[derive(Debug)]
pub struct BlockPyFunction<P: BlockPyPass, S = <P as BlockPyPass>::Expr> {
    pub function_id: FunctionId,
    pub name_gen: FunctionNameGen,
    pub names: FunctionName,
    pub kind: FunctionKind,
    pub params: ParamSpec,
    pub blocks: Vec<Block<S, P::Expr>>,
    pub doc: Option<String>,
    pub storage_layout: Option<StorageLayout>,
    pub scope: CallableScopeInfo,
}

impl<P: BlockPyPass, S: Clone> Clone for BlockPyFunction<P, S> {
    fn clone(&self) -> Self {
        Self {
            function_id: self.function_id,
            // Share the allocator state so cloned analysis/rendering snapshots
            // cannot accidentally reissue duplicate generated names.
            name_gen: self.name_gen.share(),
            names: self.names.clone(),
            kind: self.kind,
            params: self.params.clone(),
            blocks: self.blocks.clone(),
            doc: self.doc.clone(),
            storage_layout: self.storage_layout.clone(),
            scope: self.scope.clone(),
        }
    }
}

impl<P: BlockPyPass, S> BlockPyFunction<P, S> {
    pub fn lowered_kind(&self) -> &FunctionKind {
        &self.kind
    }

    pub fn storage_layout(&self) -> &Option<StorageLayout> {
        &self.storage_layout
    }

    pub fn entry_block(&self) -> &Block<S, P::Expr> {
        self.blocks
            .first()
            .expect("BlockPyFunction should have at least one block")
    }

    pub fn map_blocks<Q: BlockPyPass, T>(
        self,
        mut f: impl FnMut(Block<S, P::Expr>) -> Block<T, Q::Expr>,
    ) -> BlockPyFunction<Q, T> {
        BlockPyFunction {
            function_id: self.function_id,
            name_gen: self.name_gen,
            names: self.names,
            kind: self.kind,
            params: self.params,
            blocks: self.blocks.into_iter().map(&mut f).collect(),
            doc: self.doc,
            storage_layout: self.storage_layout,
            scope: self.scope,
        }
    }
}

pub trait NormalizedInstr {
    fn assert_blockpy_normalized(&self);
}

pub trait BlockPyPass: Clone + fmt::Debug {
    type Expr: Instr;
}

pub type InstrName<I> = <I as Instr>::Name;
pub type ResolvedStorageBlock = Block<InstrResolved>;
pub type CodegenBlock = Block<CodegenBlockPyExpr>;
pub type CodegenBlockPyFunction = BlockPyFunction<crate::passes::CodegenBlockPyPass>;
pub type CodegenBlockPyModule = BlockPyModule<crate::passes::CodegenBlockPyPass>;

pub trait BlockPyJumpTerm {
    fn jump_term(target: BlockLabel) -> Self;
}

pub trait BlockPyFallthroughTerm: BlockPyJumpTerm {
    fn implicit_function_return() -> Self;
}

pub(crate) trait ImplicitNoneExpr {
    fn implicit_none_expr() -> Self;
    fn is_implicit_none_expr(expr: &Self) -> bool;
}

pub fn assert_blockpy_block_normalized<S: NormalizedInstr, T: Instr>(block: &Block<S, T>) {
    for stmt in &block.body {
        stmt.assert_blockpy_normalized();
    }
}

#[derive(Debug, Clone)]
pub struct BlockBuilder<S, T> {
    pub body: Vec<S>,
    pub term: Option<T>,
}

pub(crate) type BlockPyStmtBuilder<I> = BlockBuilder<StructuredInstr<I>, BlockTerm<I>>;

impl<S: NormalizedInstr, T> BlockBuilder<S, T> {
    pub fn new() -> Self {
        Self {
            body: Vec::new(),
            term: None,
        }
    }

    pub fn assert_normalized(&self) {
        for stmt in &self.body {
            stmt.assert_blockpy_normalized();
        }
    }

    pub fn from_stmts(stmts: Vec<S>) -> Self {
        Self::with_term(stmts, None)
    }

    pub fn with_term(body: Vec<S>, term: impl Into<Option<T>>) -> Self {
        let builder = BlockBuilder {
            body,
            term: term.into(),
        };
        builder.assert_normalized();
        builder
    }

    pub fn push_stmt(&mut self, stmt: S) {
        assert!(
            self.term.is_none(),
            "cannot append structured BlockPy stmt after block-builder terminator"
        );
        stmt.assert_blockpy_normalized();
        self.body.push(stmt);
    }

    pub fn extend<I>(&mut self, stmts: I)
    where
        I: IntoIterator<Item = S>,
    {
        for stmt in stmts {
            self.push_stmt(stmt);
        }
    }

    pub fn set_term(&mut self, term: T) {
        assert!(
            self.term.is_none(),
            "cannot replace existing block-builder terminator"
        );
        self.term = Some(term);
    }

    pub fn finish(self) -> Self {
        self.assert_normalized();
        self
    }
}

impl<S: NormalizedInstr, T: BlockPyJumpTerm> BlockBuilder<S, T> {
    pub fn jump(target: BlockLabel) -> Self {
        Self::with_term(Vec::new(), Some(T::jump_term(target)))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum StructuredInstr<I: Instr> {
    Expr(I),
    If(StructuredIf<I>),
}

impl<I: Instr> From<I> for StructuredInstr<I> {
    fn from(value: I) -> Self {
        Self::Expr(value)
    }
}

impl<I: Instr> StructuredInstr<I> {
    pub fn assert_normalized(&self) {
        if let Self::If(if_stmt) = self {
            if_stmt.body.assert_normalized();
            if_stmt.orelse.assert_normalized();
        }
    }
}

impl<I: Instr> NormalizedInstr for StructuredInstr<I> {
    fn assert_blockpy_normalized(&self) {
        self.assert_normalized();
    }
}

impl<I> NormalizedInstr for I
where
    I: Instr,
{
    fn assert_blockpy_normalized(&self) {}
}

#[derive(Debug, Clone)]
pub enum BlockTerm<I: Instr> {
    Jump(BlockEdge),
    IfTerm(TermIf<I>),
    BranchTable(TermBranchTable<I>),
    Raise(TermRaise<I>),
    Return(I),
}

impl<I: Instr> BlockTerm<I> {
    pub fn replace_target(&mut self, from: BlockLabel, to: BlockLabel) -> bool {
        match self {
            Self::Jump(edge) => {
                if edge.target == from {
                    edge.target = to;
                    true
                } else {
                    false
                }
            }
            Self::IfTerm(if_term) => {
                let mut replaced = false;
                if if_term.then_label == from {
                    if_term.then_label = to;
                    replaced = true;
                }
                if if_term.else_label == from {
                    if_term.else_label = to;
                    replaced = true;
                }
                replaced
            }
            Self::BranchTable(branch) => {
                let mut replaced = false;
                for target in &mut branch.targets {
                    if *target == from {
                        *target = to;
                        replaced = true;
                    }
                }
                if branch.default_label == from {
                    branch.default_label = to;
                    replaced = true;
                }
                replaced
            }
            Self::Raise(_) | Self::Return(_) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StructuredIf<I: Instr> {
    pub test: I,
    pub body: BlockBuilder<StructuredInstr<I>, BlockTerm<I>>,
    pub orelse: BlockBuilder<StructuredInstr<I>, BlockTerm<I>>,
}

#[derive(Debug, Clone)]
pub struct TermIf<I: Instr> {
    pub test: I,
    pub then_label: BlockLabel,
    pub else_label: BlockLabel,
}

#[derive(Debug, Clone)]
pub struct TermBranchTable<I: Instr> {
    pub index: I,
    pub targets: Vec<BlockLabel>,
    pub default_label: BlockLabel,
}

#[derive(Debug, Clone)]
pub struct TermRaise<I: Instr> {
    pub exc: Option<I>,
}

pub fn convert_blockpy_term_expr<IIn, IOut>(value: BlockTerm<IIn>) -> BlockTerm<IOut>
where
    IIn: Instr,
    IOut: Instr + From<IIn>,
{
    struct IntoInstrMap<IIn, IOut>(std::marker::PhantomData<fn(IIn) -> IOut>);

    impl<IIn, IOut> MapInstr<IIn, IOut> for IntoInstrMap<IIn, IOut>
    where
        IIn: Instr,
        IOut: Instr + From<IIn>,
    {
        fn map_instr(&mut self, instr: IIn) -> IOut {
            instr.into()
        }

        fn map_name(&mut self, _name: IIn::Name) -> IOut::Name {
            unreachable!("BlockTerm carries no names")
        }
    }

    IntoInstrMap(std::marker::PhantomData).map_term(value)
}

#[derive(Debug, Clone)]
pub struct BlockEdge {
    pub target: BlockLabel,
    pub args: Vec<BlockArg>,
}

impl BlockEdge {
    pub fn new(target: BlockLabel) -> Self {
        Self {
            target,
            args: Vec::new(),
        }
    }

    pub fn with_args(target: BlockLabel, args: Vec<BlockArg>) -> Self {
        Self { target, args }
    }
}

#[derive(Debug, Clone)]
pub enum BlockArg {
    Name(String),
    None,
    CurrentException,
    AbruptKind(AbruptKind),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AbruptKind {
    Fallthrough,
    Return,
    Exception,
    Break,
    Continue,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BlockParamRole {
    Exception,
    AbruptKind,
    AbruptPayload,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BlockParam {
    pub name: String,
    pub role: BlockParamRole,
}

impl<I: Instr> BlockPyJumpTerm for BlockTerm<I> {
    fn jump_term(target: BlockLabel) -> Self {
        Self::Jump(BlockEdge::new(target))
    }
}

impl ImplicitNoneExpr for Expr {
    fn implicit_none_expr() -> Self {
        py_expr!("None")
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(expr, Expr::NoneLiteral(_))
    }
}

impl ImplicitNoneExpr for InstrWithAwaitAndYield {
    fn implicit_none_expr() -> Self {
        core_runtime_name_expr_with_meta("NONE", Default::default(), Default::default())
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(
            expr,
            InstrWithAwaitAndYield::Load(op)
                if op.name.is_runtime_symbol("NONE")
        )
    }
}

impl ImplicitNoneExpr for InstrWithYield {
    fn implicit_none_expr() -> Self {
        core_runtime_name_expr_with_meta("NONE", Default::default(), Default::default())
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(
            expr,
            InstrWithYield::Load(op) if op.name.is_runtime_symbol("NONE")
        )
    }
}

impl ImplicitNoneExpr for InstrLow {
    fn implicit_none_expr() -> Self {
        core_runtime_name_expr_with_meta("NONE", Default::default(), Default::default())
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(
            expr,
            InstrLow::Load(op) if op.name.is_runtime_symbol("NONE")
        )
    }
}

impl ImplicitNoneExpr for InstrResolved {
    fn implicit_none_expr() -> Self {
        Load::new(ResolvedName {
            id: "NONE".into(),
            location: NameLocation::RuntimeName,
        })
        .into()
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(
            expr,
            InstrResolved::Load(op) if op.name.is_runtime_symbol("NONE")
        )
    }
}

impl ImplicitNoneExpr for CodegenBlockPyExpr {
    fn implicit_none_expr() -> Self {
        Load::new(ResolvedName {
            id: "NONE".into(),
            location: NameLocation::RuntimeName,
        })
        .into()
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(
            expr,
            CodegenBlockPyExpr::Load(op) if op.name.is_runtime_symbol("NONE")
        )
    }
}

impl<I: Instr + ImplicitNoneExpr> BlockPyFallthroughTerm for BlockTerm<I> {
    fn implicit_function_return() -> Self {
        Self::Return(I::implicit_none_expr())
    }
}

#[cfg(test)]
mod test;
