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
pub use operation::{
    Await, BinOp, BinOpKind, Call, CallDirect, CalleeFunctionId, CellRef, CellRefForName, Del,
    DelItem, ExprAttribute, ExprBoolOp, ExprBooleanLiteral, ExprBytesLiteral, ExprCompare,
    ExprDict, ExprDictComp, ExprEllipsisLiteral, ExprFString, ExprGenerator, ExprIf,
    ExprIpyEscapeCommand, ExprLambda, ExprList, ExprListComp, ExprName, ExprNamed, ExprNoneLiteral,
    ExprNumberLiteral, ExprSet, ExprSetComp, ExprSlice, ExprStarred, ExprStringLiteral,
    ExprSubscript, ExprTString, ExprTuple, GetAttr, GetItem, Load, MakeCell, MakeFunction, SetAttr,
    SetItem, StmtAnnAssign, StmtAssert, StmtAssign, StmtAugAssign, StmtBreak, StmtClassDef,
    StmtContinue, StmtDelete, StmtExpr, StmtFor, StmtFunctionDef, StmtGlobal, StmtIf, StmtImport,
    StmtImportFrom, StmtIpyEscapeCommand, StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn,
    StmtTry, StmtTypeAlias, StmtWhile, StmtWith, Store, UnaryOp, UnaryOpKind, Yield, YieldFrom,
};
pub use ruff_python_ast::Expr;
use ruff_python_ast::{self as ast};
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
    InstrCodegen, InstrLow, InstrResolved, InstrRuff, InstrUnresolved, InstrWithAwaitAndYield,
    InstrWithYield,
};
#[allow(unused_imports)]
pub(crate) use map::{
    MapBlock, MapFunction, MapModule, MapTerm, TryMapBlock, TryMapFunction, TryMapModule,
    TryMapTerm,
};
pub use map::{MapInstr, Mappable, TryMapInstr};
pub use name_gen::{BlockLabel, FunctionId, FunctionNameGen, ModuleNameGen};
pub(crate) use validate::validate_module;
#[allow(unused_imports)]
pub(crate) use visit::{
    instr_any, walk_block, walk_block_mut, walk_expr, walk_expr_mut, walk_fn, walk_fn_mut,
    walk_module, walk_module_mut, walk_stmt, walk_stmt_mut, walk_term, walk_term_mut,
};
pub use visit::{ChildVisitable, Visit, VisitMut};

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

pub trait NameLike: Clone + fmt::Debug {
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

pub trait Instr: Clone + fmt::Debug + Sized {
    type Name: NameLike;
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

impl NameLike for UnresolvedName {
    fn id_str(&self) -> &str {
        match self {
            Self::SourceName(name) | Self::RuntimeName(name) => name.as_str(),
        }
    }

    fn is_runtime_name(&self) -> bool {
        matches!(self, Self::RuntimeName(_))
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

impl NameLike for ResolvedName {
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
pub struct Block<I: Instr> {
    pub label: BlockLabel,
    pub body: Vec<I>,
    pub term: BlockTerm<I>,
    pub params: Vec<BlockParam>,
    pub exc_edge: Option<BlockEdge>,
}

impl<I: Instr> Block<I> {
    pub fn label_str(&self) -> String {
        self.label.to_string()
    }
}

impl<I: Instr> Block<I> {
    pub fn new(
        label: BlockLabel,
        body: Vec<I>,
        term: BlockTerm<I>,
        params: Vec<BlockParam>,
        exc_edge: Option<BlockEdge>,
    ) -> Self {
        Self {
            label,
            body,
            term,
            params,
            exc_edge,
        }
    }
}

impl<I: Instr> Block<I> {
    pub(crate) fn from_builder(
        label: BlockLabel,
        builder: BlockBuilder<I>,
        params: Vec<BlockParam>,
        exc_edge: Option<BlockEdge>,
        fallthrough_target: Option<BlockLabel>,
    ) -> Self
    where
        I: ImplicitNoneExpr,
    {
        Self::new(
            label,
            builder.body,
            builder.term.unwrap_or_else(|| match fallthrough_target {
                Some(target) => BlockTerm::<I>::jump_term(target),
                None => BlockTerm::<I>::implicit_function_return(),
            }),
            params,
            exc_edge,
        )
    }
}

impl<I: Instr> Block<I> {
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
pub struct BlockPyModule<P: ModuleShape> {
    pub module_name_gen: ModuleNameGen,
    pub global_names: Vec<String>,
    pub callable_defs: Vec<BlockPyFunction<P>>,
    pub module_constants: Vec<InstrLow<ResolvedName>>,
    pub counter_defs: Vec<CounterDef>,
}

impl<P: ModuleShape> BlockPyModule<P> {
    pub fn map_callable_defs<Q: ModuleShape>(
        self,
        mut f: impl FnMut(BlockPyFunction<P>) -> BlockPyFunction<Q>,
    ) -> BlockPyModule<Q> {
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

define_operation! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}

#[derive(Clone, derive_more::From)]
pub enum Literal {
    StringLiteral(StringLiteral),
    BytesLiteral(BytesLiteral),
    NumberLiteral(NumberLiteral),
}

impl fmt::Debug for Literal {
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
        literal: Literal,
    }
}

impl LiteralValue {
    pub fn as_literal(&self) -> &Literal {
        &self.literal
    }

    pub fn into_literal(self) -> Literal {
        self.literal
    }
}

pub(crate) fn literal_value(literal: impl Into<Literal>, meta: Meta) -> LiteralValue {
    LiteralValue::new(literal.into()).with_meta(meta)
}

pub(crate) fn literal_expr<E>(literal: impl Into<Literal>, meta: Meta) -> E
where
    E: From<LiteralValue>,
{
    E::from(literal_value(literal, meta))
}

#[derive(Clone)]
pub struct StringLiteral {
    pub value: String,
}

impl fmt::Debug for StringLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

#[derive(Clone)]
pub struct BytesLiteral {
    pub value: Vec<u8>,
}

impl fmt::Debug for BytesLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

#[derive(Clone)]
pub struct NumberLiteral {
    pub value: NumberLiteralValue,
}

impl fmt::Debug for NumberLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

#[derive(Clone)]
pub enum NumberLiteralValue {
    Int(ast::Int),
    Float(f64),
}

impl fmt::Debug for NumberLiteralValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value:?}"),
        }
    }
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
    pub fn from_ast_expr_with(expr: ast::Expr, lower: impl FnOnce(ast::Expr) -> E) -> Self {
        match expr {
            ast::Expr::Starred(starred) => Self::Starred(lower(*starred.value)),
            other => Self::Positional(lower(other)),
        }
    }

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
    pub fn from_ast_keyword_with(
        keyword: ast::Keyword,
        lower: impl FnOnce(ast::Expr) -> E,
    ) -> Self {
        match keyword.arg {
            Some(arg) => Self::Named {
                arg,
                value: lower(keyword.value),
            },
            None => Self::Starred(lower(keyword.value)),
        }
    }

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
pub struct BlockPyFunction<P: ModuleShape> {
    pub function_id: FunctionId,
    pub name_gen: FunctionNameGen,
    pub names: FunctionName,
    pub kind: FunctionKind,
    pub params: ParamSpec,
    pub blocks: Vec<Block<P::Instr>>,
    pub doc: Option<String>,
    pub storage_layout: Option<StorageLayout>,
    pub scope: CallableScopeInfo,
}

impl<P: ModuleShape> Clone for BlockPyFunction<P> {
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

impl<P: ModuleShape> BlockPyFunction<P> {
    pub fn lowered_kind(&self) -> &FunctionKind {
        &self.kind
    }

    pub fn storage_layout(&self) -> &Option<StorageLayout> {
        &self.storage_layout
    }

    pub fn entry_block(&self) -> &Block<P::Instr> {
        self.blocks
            .first()
            .expect("BlockPyFunction should have at least one block")
    }

    pub fn map_blocks<Q: ModuleShape>(
        self,
        mut f: impl FnMut(Block<P::Instr>) -> Block<Q::Instr>,
    ) -> BlockPyFunction<Q> {
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

pub trait ModuleShape: Clone + fmt::Debug {
    type Instr: Instr;
}

pub type InstrName<I> = <I as Instr>::Name;
pub type ResolvedStorageBlock = Block<InstrResolved>;
pub type CodegenBlock = Block<InstrCodegen>;
pub type CodegenBlockPyFunction = BlockPyFunction<crate::passes::CodegenModuleShape>;
pub type CodegenBlockPyModule = BlockPyModule<crate::passes::CodegenModuleShape>;

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

#[derive(Debug, Clone)]
pub struct BlockBuilder<I: Instr> {
    pub body: Vec<I>,
    pub term: Option<BlockTerm<I>>,
}

impl<I: Instr> BlockBuilder<I> {
    pub fn new() -> Self {
        Self {
            body: Vec::new(),
            term: None,
        }
    }

    pub fn from_stmts(stmts: Vec<I>) -> Self {
        Self::with_term(stmts, None)
    }

    pub fn with_term(body: Vec<I>, term: impl Into<Option<BlockTerm<I>>>) -> Self {
        BlockBuilder {
            body,
            term: term.into(),
        }
    }

    pub fn push_stmt(&mut self, stmt: I) {
        assert!(
            self.term.is_none(),
            "cannot append structured BlockPy stmt after block-builder terminator"
        );
        self.body.push(stmt);
    }

    pub fn extend<It>(&mut self, stmts: It)
    where
        It: IntoIterator<Item = I>,
    {
        for stmt in stmts {
            self.push_stmt(stmt);
        }
    }

    pub fn set_term(&mut self, term: BlockTerm<I>) {
        assert!(
            self.term.is_none(),
            "cannot replace existing block-builder terminator"
        );
        self.term = Some(term);
    }

    pub fn finish(self) -> Self {
        self
    }
}

impl<I: Instr> BlockBuilder<I> {
    pub fn jump(target: BlockLabel) -> Self {
        Self::with_term(Vec::new(), Some(BlockTerm::jump_term(target)))
    }

    pub fn ensure_fallthrough_term(&mut self) {
        if self.term.is_none() {
            self.set_term(BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())));
        }
    }
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

impl ImplicitNoneExpr for InstrUnresolved {
    fn implicit_none_expr() -> Self {
        core_runtime_name_expr_with_meta("NONE", Default::default(), Default::default())
    }

    fn is_implicit_none_expr(expr: &Self) -> bool {
        matches!(
            expr,
            InstrUnresolved::Load(op) if op.name.is_runtime_symbol("NONE")
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

impl ImplicitNoneExpr for InstrCodegen {
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
            InstrCodegen::Load(op) if op.name.is_runtime_symbol("NONE")
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
