pub use self::instr::{
    Await, BinOp, BinOpKind, Call, CallDirect, CalleeFunctionId, CellRef, CellRefForName, Del,
    DelItem, ExprAttribute, ExprBoolOp, ExprBooleanLiteral, ExprBytesLiteral, ExprCompare,
    ExprDict, ExprDictComp, ExprEllipsisLiteral, ExprFString, ExprGenerator, ExprIf,
    ExprIpyEscapeCommand, ExprLambda, ExprList, ExprListComp, ExprName, ExprNamed, ExprNoneLiteral,
    ExprNumberLiteral, ExprSet, ExprSetComp, ExprSlice, ExprStarred, ExprStringLiteral,
    ExprSubscript, ExprTString, ExprTuple, GetAttr, GetItem, Load, MakeCell, MakeFunction,
    MakeFunctionWithClosure, SetAttr, SetItem, StmtAnnAssign, StmtAssert, StmtAssign,
    StmtAugAssign, StmtBreak, StmtClassDef, StmtContinue, StmtDelete, StmtExpr, StmtFor,
    StmtFunctionDef, StmtGlobal, StmtIf, StmtImport, StmtImportFrom, StmtIpyEscapeCommand,
    StmtMatch, StmtNonlocal, StmtPass, StmtRaise, StmtReturn, StmtTry, StmtTypeAlias, StmtWhile,
    StmtWith, Store, Tuple, UnaryOp, UnaryOpKind, Yield, YieldFrom,
};
#[allow(unused_imports)]
pub use self::map::{
    InstrField, MapBlock, MapFunction, MapInstr, MapModule, MapTerm, Mappable, TryMapBlock,
    TryMapFunction, TryMapInstr, TryMapModule, TryMapTerm, map_function_blocks,
    map_module_functions,
};
pub use self::meta::{
    HasMeta, HasSemanticInstrId, IdentifiedInstr, InstrId, InstrKey, Meta, WithMeta,
};
pub use self::param_specs::{Param, ParamDefaultSource, ParamKind, ParamSpec};
pub use self::scope::{
    BindingKind, BindingPurpose, BindingTarget, CallableScopeInfo, CallableScopeKind,
    CellBindingKind, CellCaptureBinding, ClassBodyFallback, ClosureInit, ClosureSlot,
    EffectiveBinding, StorageLayout, derive_effective_binding_for_name,
};
#[allow(unused_imports)]
pub use self::visit::{
    ChildVisitable, Visit, VisitMut, instr_any, walk_block, walk_block_mut, walk_expr,
    walk_expr_mut, walk_fn, walk_fn_mut, walk_module, walk_module_mut, walk_stmt, walk_stmt_mut,
    walk_term, walk_term_mut,
};
pub use crate::{define_instr, define_ruff_instr};
pub use ruff_python_ast::Expr;
use ruff_python_ast::{self as ast};
use std::fmt;
use std::fmt::Write;

mod counters;
mod instr;
mod instr_macro;
mod map;
mod meta;
mod name_gen;
mod param_specs;
mod pretty;
mod scope;
mod visit;
pub use counters::{
    CounterBranch, CounterBranchId, CounterDef, CounterId, CounterScope, CounterSite,
    DeoptEntrySource,
};
pub use name_gen::{
    BlockLabel, FunctionNameGen, LocalFunctionId, ModuleContentId, ModuleNameGen,
    PersistentFunctionId, RuntimeFunctionId, RuntimeModuleId, SerializedFunctionDebugName,
    SerializedFunctionId, SerializedIdentityTables, SerializedModuleId, SerializedModuleIdentity,
};
pub use pretty::{
    BlockPyFormat, PrettyConfig, PrettyMode, PrettyPrint, PrettyPrinter, bb_expr_text,
    blockpy_module_to_string,
};

fn is_internal_symbol(name: &str) -> bool {
    name.starts_with("_dp_") || name == "__soac__"
}

macro_rules! define_runtime_names {
    ($($variant:ident => $name:literal,)+) => {
        #[repr(u16)]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            rkyv::Archive,
            rkyv::Serialize,
            rkyv::Deserialize,
        )]
        #[rkyv(derive(Hash, PartialEq, Eq, Debug))]
        pub enum RuntimeName {
            $($variant,)+
        }

        impl RuntimeName {
            pub const ALL: &'static [Self] = &[
                $(Self::$variant,)+
            ];

            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub const fn from_id(id: u16) -> Option<Self> {
                match id {
                    $(x if x == Self::$variant as u16 => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub const fn id(self) -> u16 {
                self as u16
            }

            pub const fn name_nul_bytes(self) -> &'static [u8] {
                match self {
                    $(Self::$variant => concat!($name, "\0").as_bytes(),)+
                }
            }
        }
    };
}

define_runtime_names! {
    NoDefault => "NO_DEFAULT",
    Ellipsis => "ELLIPSIS",
    True => "TRUE",
    False => "FALSE",
    None => "NONE",
    EmptyTuple => "EMPTY_TUPLE",
    IterComplete => "ITER_COMPLETE",
    ArithmeticError => "ArithmeticError",
    AssertionError => "AssertionError",
    AttributeError => "AttributeError",
    BaseException => "BaseException",
    BaseExceptionGroup => "BaseExceptionGroup",
    BlockingIOError => "BlockingIOError",
    BrokenPipeError => "BrokenPipeError",
    BufferError => "BufferError",
    BytesWarning => "BytesWarning",
    ChildProcessError => "ChildProcessError",
    ConnectionAbortedError => "ConnectionAbortedError",
    ConnectionError => "ConnectionError",
    ConnectionRefusedError => "ConnectionRefusedError",
    ConnectionResetError => "ConnectionResetError",
    DeprecationWarning => "DeprecationWarning",
    EOFError => "EOFError",
    EncodingWarning => "EncodingWarning",
    EnvironmentError => "EnvironmentError",
    Exception => "Exception",
    ExceptionGroup => "ExceptionGroup",
    FileExistsError => "FileExistsError",
    FileNotFoundError => "FileNotFoundError",
    FloatingPointError => "FloatingPointError",
    FutureWarning => "FutureWarning",
    GeneratorExit => "GeneratorExit",
    IOError => "IOError",
    ImportError => "ImportError",
    ImportWarning => "ImportWarning",
    IndentationError => "IndentationError",
    IndexError => "IndexError",
    InterruptedError => "InterruptedError",
    IsADirectoryError => "IsADirectoryError",
    KeyError => "KeyError",
    KeyboardInterrupt => "KeyboardInterrupt",
    LookupError => "LookupError",
    MemoryError => "MemoryError",
    ModuleNotFoundError => "ModuleNotFoundError",
    NameError => "NameError",
    NotADirectoryError => "NotADirectoryError",
    NotImplemented => "NotImplemented",
    NotImplementedError => "NotImplementedError",
    OSError => "OSError",
    OverflowError => "OverflowError",
    PendingDeprecationWarning => "PendingDeprecationWarning",
    PermissionError => "PermissionError",
    ProcessLookupError => "ProcessLookupError",
    RecursionError => "RecursionError",
    ReferenceError => "ReferenceError",
    ResourceWarning => "ResourceWarning",
    RuntimeError => "RuntimeError",
    RuntimeWarning => "RuntimeWarning",
    StopAsyncIteration => "StopAsyncIteration",
    StopIteration => "StopIteration",
    SyntaxError => "SyntaxError",
    SyntaxWarning => "SyntaxWarning",
    SystemError => "SystemError",
    SystemExit => "SystemExit",
    TabError => "TabError",
    TimeoutError => "TimeoutError",
    TypeError => "TypeError",
    UnboundLocalError => "UnboundLocalError",
    UnicodeDecodeError => "UnicodeDecodeError",
    UnicodeEncodeError => "UnicodeEncodeError",
    UnicodeError => "UnicodeError",
    UnicodeTranslateError => "UnicodeTranslateError",
    UnicodeWarning => "UnicodeWarning",
    UserWarning => "UserWarning",
    ValueError => "ValueError",
    Warning => "Warning",
    ZeroDivisionError => "ZeroDivisionError",
    BuildClass => "__build_class__",
    Import => "__import__",
    Abs => "abs",
    Aiter => "aiter",
    All => "all",
    Anext => "anext",
    Any => "any",
    Ascii => "ascii",
    Bin => "bin",
    Bool => "bool",
    Breakpoint => "breakpoint",
    Builtins => "builtins",
    Bytearray => "bytearray",
    Bytes => "bytes",
    Callable => "callable",
    Chr => "chr",
    Classmethod => "classmethod",
    Compile => "compile",
    Complex => "complex",
    Copyright => "copyright",
    Credits => "credits",
    Delattr => "delattr",
    Dict => "dict",
    Dir => "dir",
    Divmod => "divmod",
    Enumerate => "enumerate",
    Eval => "eval",
    Exec => "exec",
    Exit => "exit",
    Filter => "filter",
    Float => "float",
    Format => "format",
    Frozenset => "frozenset",
    Getattr => "getattr",
    Globals => "globals",
    Hasattr => "hasattr",
    Hash => "hash",
    Help => "help",
    Hex => "hex",
    Id => "id",
    Input => "input",
    Int => "int",
    Isinstance => "isinstance",
    Issubclass => "issubclass",
    Iter => "iter",
    Len => "len",
    License => "license",
    List => "list",
    Locals => "locals",
    Map => "map",
    Max => "max",
    Memoryview => "memoryview",
    Min => "min",
    Next => "next",
    Object => "object",
    Oct => "oct",
    Open => "open",
    Ord => "ord",
    Pow => "pow",
    Print => "print",
    Property => "property",
    Quit => "quit",
    Range => "range",
    Repr => "repr",
    Reversed => "reversed",
    Round => "round",
    Set => "set",
    Setattr => "setattr",
    Slice => "slice",
    Sorted => "sorted",
    Staticmethod => "staticmethod",
    Str => "str",
    Sum => "sum",
    Tuple => "tuple",
    Type => "type",
    Vars => "vars",
    Zip => "zip",
    ImportUnderscore => "import_",
    ImportAttr => "import_attr",
    Index => "_index",
    UnsupportedFrameBuiltin => "_unsupported_frame_builtin",
    TupleFromIter => "tuple_from_iter",
    EvalStringLiteral => "eval_string_literal",
    Deepcopy => "__deepcopy__",
    TypingGeneric => "typing_Generic",
    TypingTypeVar => "typing_TypeVar",
    TypingTypeVarTuple => "typing_TypeVarTuple",
    TypingParamSpec => "typing_ParamSpec",
    TypingTypeAliasType => "typing_TypeAliasType",
    TypingUnpack => "typing_Unpack",
    TemplatelibTemplate => "templatelib_Template",
    TemplatelibInterpolation => "templatelib_Interpolation",
    RaiseDeletedName => "raise_deleted_name",
    BbTraceEnter => "bb_trace_enter",
    YieldfromCellValue => "_yieldfrom_cell_value",
    CurrentYieldfrom => "_current_yieldfrom",
    IsCancelledError => "_is_cancelled_error",
    ReraiseControlFlow => "_reraise_control_flow",
    ClearCell => "_clear_cell",
    MarkClosed => "_mark_closed",
    NormalizeThrowExc => "_normalize_throw_exc",
    CurrentThrowContext => "_current_throw_context",
    FloatFromLiteral => "float_from_literal",
    ComplexFromParts => "complex_from_parts",
    ClassLookupCell => "class_lookup_cell",
    ClassLookupGlobal => "class_lookup_global",
    ValidateExceptionType => "_validate_exception_type",
    ExceptionMatches => "exception_matches",
    ExceptiongroupSplit => "exceptiongroup_split",
    Unpack => "unpack",
    CallSuper => "call_super",
    CallSuperNoargs => "call_super_noargs",
    MatchClassValidateArity => "_match_class_validate_arity",
    MatchClassAttrExists => "match_class_attr_exists",
    MatchClassAttrValue => "match_class_attr_value",
    CodeTemplateGen => "code_template_gen",
    CodeTemplateAsyncGen => "code_template_async_gen",
    AnnotationForwardrefValue => "annotation_forwardref_value",
    CurrentException => "current_exception",
    CreateClass => "create_class",
    ExcInfo => "exc_info",
    ExcInfoFromException => "exc_info_from_exception",
    GetAwaitableIter => "_get_awaitable_iter",
    AwaitIter => "await_iter",
    AnextOrSentinel => "anext_or_sentinel",
    NextOrSentinel => "next_or_sentinel",
    RaiseFrom => "raise_from",
    CallExceptionClass => "_call_exception_class",
    ImportStar => "import_star",
    StoreGlobal => "store_global",
    CellRef => "cell_ref",
    DelQuietly => "del_quietly",
    MakeFunction => "make_function",
    LookupSpecialMethod => "_lookup_special_method",
    HasSpecialMethod => "_has_special_method",
    MissingContextProtocolMessage => "_missing_context_protocol_message",
    ContextmanagerEnter => "contextmanager_enter",
    ContextmanagerGetExit => "contextmanager_get_exit",
    ContextmanagerExit => "contextmanager_exit",
    EnsureAwaitable => "_ensure_awaitable",
    AsynccontextmanagerAenter => "asynccontextmanager_aenter",
    AsynccontextmanagerGetAexit => "asynccontextmanager_get_aexit",
    AsynccontextmanagerExit => "asynccontextmanager_exit",
    IterRange => "IterRange",
    AsyncGenComplete => "AsyncGenComplete",
    ClosureGenerator => "ClosureGenerator",
    Coroutine => "Coroutine",
    ClosureAsyncGenerator => "ClosureAsyncGenerator",
    AsyncGenSend => "AsyncGenSend",
    AwaitIterWrapper => "_AwaitIterWrapper",
    DynamicCallee => "__dp_dynamic_callee",
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct LocalLocation(pub u32);

impl LocalLocation {
    pub fn slot(self) -> u32 {
        self.0
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct GlobalSlot(pub u32);

impl GlobalSlot {
    pub fn slot(self) -> u32 {
        self.0
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum NameLocation {
    Local(LocalLocation),
    GlobalName,
    Global(GlobalSlot),
    RuntimeName(RuntimeName),
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

    pub fn global_name() -> Self {
        Self::GlobalName
    }

    pub fn runtime_name(name: RuntimeName) -> Self {
        Self::RuntimeName(name)
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
            Self::GlobalName
            | Self::Global(_)
            | Self::RuntimeName(_)
            | Self::Cell(_)
            | Self::Constant(_) => None,
        }
    }

    pub fn as_cell(self) -> Option<CellLocation> {
        match self {
            Self::Cell(location) => Some(location),
            Self::Local(_)
            | Self::GlobalName
            | Self::Global(_)
            | Self::RuntimeName(_)
            | Self::Constant(_) => None,
        }
    }

    pub fn as_constant(self) -> Option<u32> {
        match self {
            Self::Constant(index) => Some(index),
            Self::Local(_)
            | Self::GlobalName
            | Self::Global(_)
            | Self::RuntimeName(_)
            | Self::Cell(_) => None,
        }
    }

    pub fn as_global(self) -> Option<GlobalSlot> {
        match self {
            Self::Global(slot) => Some(slot),
            Self::Local(_)
            | Self::GlobalName
            | Self::RuntimeName(_)
            | Self::Cell(_)
            | Self::Constant(_) => None,
        }
    }

    pub fn is_global(self) -> bool {
        matches!(self, Self::Global(_))
    }

    pub fn is_global_name(self) -> bool {
        matches!(self, Self::GlobalName)
    }

    pub fn is_runtime_name(self) -> bool {
        matches!(self, Self::RuntimeName(_))
    }

    pub fn runtime_name_id(self) -> Option<RuntimeName> {
        match self {
            Self::RuntimeName(name) => Some(name),
            Self::Local(_)
            | Self::GlobalName
            | Self::Global(_)
            | Self::Cell(_)
            | Self::Constant(_) => None,
        }
    }

    pub fn pretty_id(self, unresolved_name: &str) -> String {
        match self {
            Self::Local(location) => format!("{location:?}"),
            Self::GlobalName => format!("{unresolved_name}@global"),
            Self::Global(slot) => format!("{unresolved_name}@g{}", slot.slot()),
            Self::RuntimeName(name) => name.name().to_string(),
            Self::Cell(location) => format!("{location:?}"),
            Self::Constant(index) => format!("constant slot {index}"),
        }
    }
}

pub trait NameLike: Clone + fmt::Debug {
    fn id_str(&self) -> &str;
    fn runtime_name(name: &str) -> Self;
    fn runtime_name_id(&self) -> Option<RuntimeName> {
        None
    }
    fn pretty_id(&self) -> String {
        self.id_str().to_string()
    }
    fn is_runtime_name(&self) -> bool {
        self.runtime_name_id().is_some()
    }
    fn is_runtime_symbol(&self, name: &str) -> bool {
        self.runtime_name_id() == RuntimeName::from_name(name)
    }
}

pub trait Instr: Clone + fmt::Debug + Sized {
    type Name: NameLike;
    type Extra: Clone + fmt::Debug + Default;
}

pub trait InstrWithConstantNone: Instr {
    fn constant_none() -> Self;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BlockPyName {
    id: String,
}

impl BlockPyName {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn as_str(&self) -> &str {
        self.id.as_str()
    }

    pub fn into_ast_name(self) -> ast::name::Name {
        ast::name::Name::new(self.id)
    }
}

impl fmt::Display for BlockPyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for BlockPyName {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for BlockPyName {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<ast::name::Name> for BlockPyName {
    fn from(value: ast::name::Name) -> Self {
        Self::new(value.as_str())
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum UnresolvedName {
    SourceName(BlockPyName),
    RuntimeName(RuntimeName),
}

impl fmt::Debug for UnresolvedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pretty_id())
    }
}

impl NameLike for UnresolvedName {
    fn id_str(&self) -> &str {
        match self {
            Self::SourceName(name) => name.as_str(),
            Self::RuntimeName(name) => name.name(),
        }
    }

    fn runtime_name(name: &str) -> Self {
        Self::RuntimeName(
            RuntimeName::from_name(name)
                .unwrap_or_else(|| panic!("unknown SOAC runtime name {name:?}")),
        )
    }

    fn runtime_name_id(&self) -> Option<RuntimeName> {
        match self {
            Self::RuntimeName(name) => Some(*name),
            Self::SourceName(_) => None,
        }
    }
}

impl<T> From<T> for UnresolvedName
where
    T: Into<BlockPyName>,
{
    fn from(value: T) -> Self {
        Self::SourceName(value.into())
    }
}

impl UnresolvedName {
    pub fn name(self) -> ast::name::Name {
        match self {
            Self::SourceName(name) => name.into_ast_name(),
            Self::RuntimeName(name) => ast::name::Name::new(name.name()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ResolvedName {
    pub id: BlockPyName,
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

    pub fn runtime_name_id(&self) -> Option<RuntimeName> {
        self.location.runtime_name_id()
    }
}

impl NameLike for ResolvedName {
    fn id_str(&self) -> &str {
        self.runtime_name_id()
            .map(RuntimeName::name)
            .unwrap_or_else(|| self.id.as_str())
    }

    fn runtime_name(name: &str) -> Self {
        let runtime_name = RuntimeName::from_name(name)
            .unwrap_or_else(|| panic!("unknown SOAC runtime name {name:?}"));
        Self {
            id: BlockPyName::new(runtime_name.name()),
            location: NameLocation::RuntimeName(runtime_name),
        }
    }

    fn pretty_id(&self) -> String {
        self.resolved_pretty_id()
    }

    fn runtime_name_id(&self) -> Option<RuntimeName> {
        self.runtime_name_id()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FunctionKind {
    Function,
    Coroutine,
    Generator,
    AsyncGenerator,
}

impl FunctionKind {
    pub const fn make_function_kind_name(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Coroutine => "coroutine",
            Self::Generator => "generator",
            Self::AsyncGenerator => "async_generator",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FunctionExecutionMode {
    Jit,
    Interpreted,
}

impl Default for FunctionExecutionMode {
    fn default() -> Self {
        Self::Jit
    }
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    pub fn from_builder(
        label: BlockLabel,
        builder: BlockBuilder<I>,
        params: Vec<BlockParam>,
        exc_edge: Option<BlockEdge>,
        fallthrough_target: Option<BlockLabel>,
    ) -> Self
    where
        I: InstrWithConstantNone,
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

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BlockPyModule<P: ModuleShape> {
    #[rkyv(with = rkyv::with::Skip)]
    pub module_name_gen: ModuleNameGen,
    pub global_names: Vec<String>,
    pub callable_defs: Vec<BlockPyFunction<P>>,
    pub module_constants: Vec<P::ModuleConstant>,
    pub counter_defs: Vec<CounterDef>,
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CallArgPositional<E> {
    Positional(E),
    Starred(E),
}

impl<E> PrettyPrint for CallArgPositional<E>
where
    E: PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        match self {
            Self::Positional(expr) => {
                printer.write_str("Positional(")?;
                expr.fmt_pretty(printer)?;
                printer.write_char(')')
            }
            Self::Starred(expr) => {
                printer.write_str("Starred(")?;
                expr.fmt_pretty(printer)?;
                printer.write_char(')')
            }
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct KeywordName {
    id: String,
}

impl KeywordName {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn as_str(&self) -> &str {
        self.id.as_str()
    }

    pub fn into_ast_identifier(self, range: ruff_text_size::TextRange) -> ast::Identifier {
        ast::Identifier::new(self.id, range)
    }
}

impl fmt::Display for KeywordName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for KeywordName {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for KeywordName {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<ast::Identifier> for KeywordName {
    fn from(value: ast::Identifier) -> Self {
        Self::new(value.id.as_str())
    }
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CallArgKeyword<E> {
    Named { arg: KeywordName, value: E },
    Starred(E),
}

impl<E> PrettyPrint for CallArgKeyword<E>
where
    E: PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        match self {
            Self::Named { arg, value } => {
                write!(printer, "Named {{ arg: {arg:?}, value: ")?;
                value.fmt_pretty(printer)?;
                printer.write_str(" }")
            }
            Self::Starred(value) => {
                printer.write_str("Starred(")?;
                value.fmt_pretty(printer)?;
                printer.write_char(')')
            }
        }
    }
}

impl<E> CallArgKeyword<E> {
    pub fn from_ast_keyword_with(
        keyword: ast::Keyword,
        lower: impl FnOnce(ast::Expr) -> E,
    ) -> Self {
        match keyword.arg {
            Some(arg) => Self::Named {
                arg: arg.into(),
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

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BlockPyFunction<P: ModuleShape> {
    pub function_id: RuntimeFunctionId,
    #[rkyv(with = rkyv::with::Skip)]
    pub name_gen: FunctionNameGen,
    pub names: FunctionName,
    pub kind: FunctionKind,
    pub execution_mode: FunctionExecutionMode,
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
            execution_mode: self.execution_mode,
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

    pub fn execution_mode(&self) -> FunctionExecutionMode {
        self.execution_mode
    }

    pub fn storage_layout(&self) -> &Option<StorageLayout> {
        &self.storage_layout
    }

    pub fn entry_block(&self) -> &Block<P::Instr> {
        self.blocks
            .first()
            .expect("BlockPyFunction should have at least one block")
    }
}

pub trait ModuleShape: Clone + fmt::Debug {
    type Instr: Instr;
    type ModuleConstant: Clone + fmt::Debug;
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BlockTerm<I: Instr> {
    Jump(BlockEdge),
    IfTerm(TermIf<I>),
    BranchTable(TermBranchTable<I>),
    Raise(TermRaise<I>),
    Return(I),
}

impl<I: Instr> BlockTerm<I> {
    pub fn jump_term(target: BlockLabel) -> Self {
        Self::Jump(BlockEdge::new(target))
    }
}

impl<I: InstrWithConstantNone> BlockTerm<I> {
    pub fn implicit_function_return() -> Self {
        Self::Return(I::constant_none())
    }
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TermIf<I: Instr> {
    pub test: I,
    pub then_label: BlockLabel,
    pub else_label: BlockLabel,
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TermBranchTable<I: Instr> {
    pub index: I,
    pub targets: Vec<BlockLabel>,
    pub default_label: BlockLabel,
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TermRaise<I: Instr> {
    pub exc: Option<I>,
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BlockArg {
    Name(String),
    None,
    CurrentException,
    AbruptKind(AbruptKind),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum AbruptKind {
    Fallthrough,
    Return,
    Exception,
    Break,
    Continue,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BlockParamRole {
    Exception,
    AbruptKind,
    AbruptPayload,
}

#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BlockParam {
    pub name: String,
    pub role: BlockParamRole,
}
