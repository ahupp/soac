use super::instr_macro::{define_instr, define_ruff_instr};
use super::{
    BlockPyName, CallArgKeyword, CallArgPositional, CellBindingKind, CellLocation, ChildVisitable,
    FunctionKind, HasMeta, Instr, InstrField, MapInstr, Mappable, Meta, NameLike, PrettyPrint,
    PrettyPrinter, RuntimeFunctionId, TryMapInstr, Visit, VisitMut, WithMeta,
};
use ruff_python_ast::{self as ast};
use soac_contracts::SourceIdentity;
use std::fmt;
use std::fmt::Write;

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    MatMul,
    TrueDiv,
    FloorDiv,
    Mod,
    Pow,
    LShift,
    RShift,
    Or,
    Xor,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Source order: the left operand is the needle, the right is the container.
    /// Only the native PySequence_Contains call reverses their ABI order.
    Contains,
    Is,
    InplaceAdd,
    InplaceSub,
    InplaceMul,
    InplaceMatMul,
    InplaceTrueDiv,
    InplaceFloorDiv,
    InplaceMod,
    InplacePow,
    InplaceLShift,
    InplaceRShift,
    InplaceOr,
    InplaceXor,
    InplaceAnd,
}

impl BinOpKind {
    pub fn from_ast_operator(op: ast::Operator) -> Self {
        match op {
            ast::Operator::Add => Self::Add,
            ast::Operator::Sub => Self::Sub,
            ast::Operator::Mult => Self::Mul,
            ast::Operator::MatMult => Self::MatMul,
            ast::Operator::Div => Self::TrueDiv,
            ast::Operator::Mod => Self::Mod,
            ast::Operator::Pow => Self::Pow,
            ast::Operator::LShift => Self::LShift,
            ast::Operator::RShift => Self::RShift,
            ast::Operator::BitOr => Self::Or,
            ast::Operator::BitXor => Self::Xor,
            ast::Operator::BitAnd => Self::And,
            ast::Operator::FloorDiv => Self::FloorDiv,
        }
    }

    pub fn from_ast_inplace_operator(op: ast::Operator) -> Self {
        match op {
            ast::Operator::Add => Self::InplaceAdd,
            ast::Operator::Sub => Self::InplaceSub,
            ast::Operator::Mult => Self::InplaceMul,
            ast::Operator::MatMult => Self::InplaceMatMul,
            ast::Operator::Div => Self::InplaceTrueDiv,
            ast::Operator::Mod => Self::InplaceMod,
            ast::Operator::Pow => Self::InplacePow,
            ast::Operator::LShift => Self::InplaceLShift,
            ast::Operator::RShift => Self::InplaceRShift,
            ast::Operator::BitOr => Self::InplaceOr,
            ast::Operator::BitXor => Self::InplaceXor,
            ast::Operator::BitAnd => Self::InplaceAnd,
            ast::Operator::FloorDiv => Self::InplaceFloorDiv,
        }
    }

    pub fn into_ast_operator(self) -> ast::Operator {
        match self {
            Self::Add | Self::InplaceAdd => ast::Operator::Add,
            Self::Sub | Self::InplaceSub => ast::Operator::Sub,
            Self::Mul | Self::InplaceMul => ast::Operator::Mult,
            Self::MatMul | Self::InplaceMatMul => ast::Operator::MatMult,
            Self::TrueDiv | Self::InplaceTrueDiv => ast::Operator::Div,
            Self::FloorDiv | Self::InplaceFloorDiv => ast::Operator::FloorDiv,
            Self::Mod | Self::InplaceMod => ast::Operator::Mod,
            Self::Pow | Self::InplacePow => ast::Operator::Pow,
            Self::LShift | Self::InplaceLShift => ast::Operator::LShift,
            Self::RShift | Self::InplaceRShift => ast::Operator::RShift,
            Self::Or | Self::InplaceOr => ast::Operator::BitOr,
            Self::Xor | Self::InplaceXor => ast::Operator::BitXor,
            Self::And | Self::InplaceAnd => ast::Operator::BitAnd,
            other => panic!("comparison-only BinOpKind has no ast::Operator: {other:?}"),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum UnaryOpKind {
    Pos,
    Neg,
    Invert,
    Not,
    Truth,
}

impl UnaryOpKind {
    pub fn from_ast_unary_op(op: ast::UnaryOp) -> Self {
        match op {
            ast::UnaryOp::Not => Self::Not,
            ast::UnaryOp::Invert => Self::Invert,
            ast::UnaryOp::USub => Self::Neg,
            ast::UnaryOp::UAdd => Self::Pos,
        }
    }

    pub fn into_ast_unary_op(self) -> ast::UnaryOp {
        match self {
            Self::Pos => ast::UnaryOp::UAdd,
            Self::Neg => ast::UnaryOp::USub,
            Self::Invert => ast::UnaryOp::Invert,
            Self::Not | Self::Truth => ast::UnaryOp::Not,
        }
    }
}

/// The source activation's materialized namespace at one call operation.
/// A module uses its defining environment; a class carries its exact mapping.
/// The enclosing optional field is None for unmaterialized function locals.
/// This choice stays on the operation when its surrounding blocks are inlined.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FrameNamespace<E> {
    ModuleGlobals,
    Mapping(Box<E>),
}

impl<E> FrameNamespace<E> {
    pub fn mapping(&self) -> Option<&E> {
        match self {
            Self::ModuleGlobals => None,
            Self::Mapping(value) => Some(value),
        }
    }

    pub fn mapping_mut(&mut self) -> Option<&mut E> {
        match self {
            Self::ModuleGlobals => None,
            Self::Mapping(value) => Some(value),
        }
    }

    pub fn map_instr<T>(self, map: impl FnOnce(E) -> T) -> FrameNamespace<T> {
        match self {
            Self::ModuleGlobals => FrameNamespace::ModuleGlobals,
            Self::Mapping(value) => FrameNamespace::Mapping(Box::new(map(*value))),
        }
    }

    pub fn try_map_instr<T, Error>(
        self,
        map: impl FnOnce(E) -> Result<T, Error>,
    ) -> Result<FrameNamespace<T>, Error> {
        match self {
            Self::ModuleGlobals => Ok(FrameNamespace::ModuleGlobals),
            Self::Mapping(value) => {
                map(*value).map(|value| FrameNamespace::Mapping(Box::new(value)))
            }
        }
    }
}

impl<E: PrettyPrint> PrettyPrint for FrameNamespace<E> {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        match self {
            Self::ModuleGlobals => printer.write_str("ModuleGlobals"),
            Self::Mapping(value) => value.fmt_pretty(printer),
        }
    }
}

define_instr! {
    /// Evaluate one source-selected class decorator preparation before its
    /// class is allocated. Factory arguments remain ordinary Python values;
    /// the runtime must authenticate the actual callable and its invocation.
    /// A failed proposal may still carry the ordinary decorator without any
    /// class-construction authority.
    pub struct PrepareClassDecorator<E> {
        definition: SourceIdentity,
        construction_function: RuntimeFunctionId,
        decorator: Box<E>,
        args: Vec<CallArgPositional<E>>,
        keywords: Vec<CallArgKeyword<E>>,
        factory: bool,
        frame_namespace: Option<FrameNamespace<E>>,
    }
}

impl<E: Instr> PrepareClassDecorator<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        std::iter::once(self.decorator.as_ref())
            .chain(self.args.iter().map(CallArgPositional::expr))
            .chain(self.keywords.iter().map(CallArgKeyword::expr))
            .chain(
                self.frame_namespace
                    .as_ref()
                    .and_then(FrameNamespace::mapping),
            )
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        std::iter::once(self.decorator.as_mut())
            .chain(self.args.iter_mut().map(CallArgPositional::expr_mut))
            .chain(self.keywords.iter_mut().map(CallArgKeyword::expr_mut))
            .chain(
                self.frame_namespace
                    .as_mut()
                    .and_then(FrameNamespace::mapping_mut),
            )
    }
}

define_instr! {
    /// Apply the actual prepared decorator once, after native class callbacks,
    /// and complete its transformation before the source class binding. The
    /// enclosing cleanup region discards the preparation after this operation's
    /// argument cleanup, preserving argument-before-callable release order.
    pub struct ApplyClassDecorator<E> {
        definition: SourceIdentity,
        construction_function: RuntimeFunctionId,
        preparation: Box<E>,
        class: Box<E>,
        frame_namespace: Option<FrameNamespace<E>>,
    }
}

impl<E: Instr> ApplyClassDecorator<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        [self.preparation.as_ref(), self.class.as_ref()]
            .into_iter()
            .chain(
                self.frame_namespace
                    .as_ref()
                    .and_then(FrameNamespace::mapping),
            )
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        [self.preparation.as_mut(), self.class.as_mut()]
            .into_iter()
            .chain(
                self.frame_namespace
                    .as_mut()
                    .and_then(FrameNamespace::mapping_mut),
            )
    }
}

define_instr! {
    /// Finish a class-decorator operand's lifetime on every exit from its
    /// construction/application region. This clears the private carrier even
    /// if that carrier escaped; it does not authenticate or adopt a class.
    pub struct DiscardClassDecorator<E> {
        preparation: Box<E>,
    }
}

impl<E: Instr> DiscardClassDecorator<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        std::iter::once(self.preparation.as_ref())
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        std::iter::once(self.preparation.as_mut())
    }
}

define_instr! {
    pub struct BinOp<E> {
        kind: BinOpKind,
        left: Box<E>,
        right: Box<E>,
    }
}

define_instr! {
    pub struct UnaryOp<E> {
        kind: UnaryOpKind,
        operand: Box<E>,
    }
}

define_instr! {
    pub struct Tuple<E> {
        values: Vec<E>,
    }
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Call<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub func: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
    /// Explicit module-global or class namespace for namespace-sensitive builtins.
    /// Function locals remain None; modules select their actual environment,
    /// and classes carry a resolved mapping operand, independent of the callee.
    pub frame_namespace: Option<FrameNamespace<E>>,
}

impl<E> PrettyPrint for Call<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        self.func.as_ref().fmt_pretty(printer)?;
        printer.write_char('(')?;
        let mut first = true;
        for arg in &self.args {
            if !first {
                printer.write_str(", ")?;
            }
            first = false;
            match arg {
                CallArgPositional::Positional(expr) => expr.fmt_pretty(printer)?,
                CallArgPositional::Starred(expr) => {
                    printer.write_char('*')?;
                    expr.fmt_pretty(printer)?;
                }
            }
        }
        for keyword in &self.keywords {
            if !first {
                printer.write_str(", ")?;
            }
            first = false;
            match keyword {
                CallArgKeyword::Named { arg, value } => {
                    write!(printer, "{arg}=")?;
                    value.fmt_pretty(printer)?;
                }
                CallArgKeyword::Starred(value) => {
                    printer.write_str("**")?;
                    value.fmt_pretty(printer)?;
                }
            }
        }
        printer.write_char(')')
    }
}

impl<E: Instr> Call<E> {
    pub fn new(
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
        }
    }

    pub fn with_frame_namespace(mut self, namespace: Option<FrameNamespace<E>>) -> Self {
        self.frame_namespace = namespace;
        self
    }

    pub fn with_extra(mut self, extra: E::Extra) -> Self {
        self.extra = extra;
        self
    }

    pub fn extra(&self) -> &E::Extra {
        &self.extra
    }

    pub fn extra_mut(&mut self) -> &mut E::Extra {
        &mut self.extra
    }
}

impl<E: Instr> HasMeta for Call<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for Call<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for Call<E>
where
    E: Instr + ChildVisitable<E>,
{
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
        if let Some(FrameNamespace::Mapping(namespace)) = &mut self.frame_namespace {
            visitor.visit_instr_mut(namespace);
        }
    }

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
        if let Some(FrameNamespace::Mapping(namespace)) = &self.frame_namespace {
            visitor.visit_instr(namespace);
        }
    }
}

impl<E: Instr> Mappable<E> for Call<E> {
    type Mapped<T: Instr> = Call<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        Call {
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
        Ok(Call {
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
        Call {
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
            frame_namespace: self
                .frame_namespace
                .map(|value| value.map_instr(|value| map.map_instr(value))),
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(Call {
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
            frame_namespace: self
                .frame_namespace
                .map(|value| value.try_map_instr(|value| map.try_map_instr(value)))
                .transpose()?,
        })
    }
}

define_instr! {
    pub struct CalleeFunctionId<E> {
        value: Box<E>,
    }
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CallDirect<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub callable: Box<E>,
    pub function_id: RuntimeFunctionId,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
}

impl<E> PrettyPrint for CallDirect<E>
where
    E: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        write!(printer, "CallDirect({}, ", self.function_id)?;
        self.callable.as_ref().fmt_pretty(printer)?;
        for arg in &self.args {
            printer.write_str(", ")?;
            match arg {
                CallArgPositional::Positional(expr) => expr.fmt_pretty(printer)?,
                CallArgPositional::Starred(expr) => {
                    printer.write_char('*')?;
                    expr.fmt_pretty(printer)?;
                }
            }
        }
        for keyword in &self.keywords {
            printer.write_str(", ")?;
            match keyword {
                CallArgKeyword::Named { arg, value } => {
                    write!(printer, "{arg}=")?;
                    value.fmt_pretty(printer)?;
                }
                CallArgKeyword::Starred(value) => {
                    printer.write_str("**")?;
                    value.fmt_pretty(printer)?;
                }
            }
        }
        printer.write_char(')')
    }
}

impl<E: Instr> CallDirect<E> {
    pub fn new(
        callable: impl Into<Box<E>>,
        function_id: RuntimeFunctionId,
        args: impl Into<Vec<CallArgPositional<E>>>,
        keywords: impl Into<Vec<CallArgKeyword<E>>>,
    ) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            callable: callable.into(),
            function_id,
            args: args.into(),
            keywords: keywords.into(),
        }
    }

    pub fn with_extra(mut self, extra: E::Extra) -> Self {
        self.extra = extra;
        self
    }

    pub fn extra(&self) -> &E::Extra {
        &self.extra
    }

    pub fn extra_mut(&mut self) -> &mut E::Extra {
        &mut self.extra
    }
}

impl<E: Instr> HasMeta for CallDirect<E> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<E: Instr> WithMeta for CallDirect<E> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E> ChildVisitable<E> for CallDirect<E>
where
    E: Instr + ChildVisitable<E>,
{
    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<E> + ?Sized,
    {
        visitor.visit_instr_mut(self.callable.as_mut());
        for arg in &mut self.args {
            visitor.visit_instr_mut(arg.expr_mut());
        }
        for keyword in &mut self.keywords {
            visitor.visit_instr_mut(keyword.expr_mut());
        }
    }

    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<E> + ?Sized,
    {
        visitor.visit_instr(self.callable.as_ref());
        for arg in &self.args {
            visitor.visit_instr(arg.expr());
        }
        for keyword in &self.keywords {
            visitor.visit_instr(keyword.expr());
        }
    }
}

impl<E: Instr> Mappable<E> for CallDirect<E> {
    type Mapped<T: Instr> = CallDirect<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        CallDirect {
            _meta: self._meta,
            extra: Default::default(),
            callable: map.map_instr(*self.callable).into(),
            function_id: self.function_id,
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
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(CallDirect {
            _meta: self._meta,
            extra: Default::default(),
            callable: map.try_map_instr(*self.callable)?.into(),
            function_id: self.function_id,
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
        })
    }

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        CallDirect {
            _meta: self._meta,
            extra: self.extra,
            callable: map.map_instr(*self.callable).into(),
            function_id: self.function_id,
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
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        Ok(CallDirect {
            _meta: self._meta,
            extra: self.extra,
            callable: map.try_map_instr(*self.callable)?.into(),
            function_id: self.function_id,
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
        })
    }
}

define_instr! {
    pub struct GetAttr<E> {
        value: Box<E>,
        attr: Box<E>,
    }
}

define_instr! {
    pub struct SetAttr<E> {
        value: Box<E>,
        attr: Box<E>,
        replacement: Box<E>,
    }
}

define_instr! {
    pub struct GetItem<E> {
        value: Box<E>,
        index: Box<E>,
    }
}

define_instr! {
    pub struct SetItem<E> {
        value: Box<E>,
        index: Box<E>,
        replacement: Box<E>,
    }
}

impl<E: Instr> SetItem<E> {
    /// Release captured inputs in native STORE_SUBSCR stack-pop order:
    /// index, container, replacement. Indices refer to this operation's
    /// value/index/replacement field order. Borrowed inputs are not released;
    /// a transferred replacement has already left the operation's ownership.
    pub const INPUT_RELEASE_ORDER: [usize; 3] = [1, 0, 2];
}

define_instr! {
    pub struct DelItem<E> {
        value: Box<E>,
        index: Box<E>,
    }
}

/// The source binding whose value a cell load reads. This remains unchanged
/// when inlining remaps a captured cell to an owned or preserved storage slot:
/// an empty free-variable read still raises NameError, not UnboundLocalError.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CellLoadBinding {
    pub logical_name: BlockPyName,
    pub kind: CellBindingKind,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Load<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
    /// Required for a resolved cell-value load; absent for all other loads.
    /// Name binding selects this once, independently of physical CellLocation.
    pub cell_binding: Option<CellLoadBinding>,
}

impl<I: Instr> PrettyPrint for Load<I> {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        printer.write_str(&self.name.pretty_id())
    }
}

impl<I: Instr> Load<I> {
    pub fn new(name: impl Into<I::Name>) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            name: name.into(),
            cell_binding: None,
        }
    }

    pub fn with_cell_binding(mut self, cell_binding: Option<CellLoadBinding>) -> Self {
        self.cell_binding = cell_binding;
        self
    }

    pub fn with_extra(mut self, extra: I::Extra) -> Self {
        self.extra = extra;
        self
    }

    pub fn extra(&self) -> &I::Extra {
        &self.extra
    }

    pub fn extra_mut(&mut self) -> &mut I::Extra {
        &mut self.extra
    }
}

impl<I: Instr> HasMeta for Load<I> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<I: Instr> WithMeta for Load<I> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<I> ChildVisitable<I> for Load<I>
where
    I: Instr + ChildVisitable<I>,
{
    fn visit_children<V>(&self, _visitor: &mut V)
    where
        V: crate::block_py::Visit<I> + ?Sized,
    {
    }

    fn visit_children_mut<V>(&mut self, _visitor: &mut V)
    where
        V: crate::block_py::VisitMut<I> + ?Sized,
    {
    }
}

impl<I: Instr> Mappable<I> for Load<I> {
    type Mapped<T: Instr> = Load<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<I, T>,
    {
        Load {
            _meta: self._meta,
            extra: Default::default(),
            name: map.map_name(self.name),
            cell_binding: self.cell_binding,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<I, T, Error>,
    {
        Ok(Load {
            _meta: self._meta,
            extra: Default::default(),
            name: map.try_map_name(self.name)?,
            cell_binding: self.cell_binding,
        })
    }

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<I>
    where
        M: MapInstr<I, I>,
    {
        Load {
            _meta: self._meta,
            extra: self.extra,
            name: map.map_name(self.name),
            cell_binding: self.cell_binding,
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<I>, Error>
    where
        M: TryMapInstr<I, I, Error>,
    {
        Ok(Load {
            _meta: self._meta,
            extra: self.extra,
            name: map.try_map_name(self.name)?,
            cell_binding: self.cell_binding,
        })
    }
}

/// Move one compiler operand owner out of its slot and clear the slot.
/// This is a checked, effectful read; it never clones the object.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TakeOperand<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
}

impl<I: Instr> PrettyPrint for TakeOperand<I> {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        printer.write_str("take_operand(")?;
        printer.write_str(&self.name.pretty_id())?;
        printer.write_str(")")
    }
}

impl<I: Instr> TakeOperand<I> {
    pub fn new(name: impl Into<I::Name>) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            name: name.into(),
        }
    }

    pub fn with_extra(mut self, extra: I::Extra) -> Self {
        self.extra = extra;
        self
    }

    pub fn extra(&self) -> &I::Extra {
        &self.extra
    }

    pub fn extra_mut(&mut self) -> &mut I::Extra {
        &mut self.extra
    }
}

impl<I: Instr> HasMeta for TakeOperand<I> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<I: Instr> WithMeta for TakeOperand<I> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<I> ChildVisitable<I> for TakeOperand<I>
where
    I: Instr + ChildVisitable<I>,
{
    fn visit_children<V>(&self, _visitor: &mut V)
    where
        V: crate::block_py::Visit<I> + ?Sized,
    {
    }

    fn visit_children_mut<V>(&mut self, _visitor: &mut V)
    where
        V: crate::block_py::VisitMut<I> + ?Sized,
    {
    }
}

impl<I: Instr> Mappable<I> for TakeOperand<I> {
    type Mapped<T: Instr> = TakeOperand<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<I, T>,
    {
        TakeOperand {
            _meta: self._meta,
            extra: Default::default(),
            name: map.map_name(self.name),
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<I, T, Error>,
    {
        Ok(TakeOperand {
            _meta: self._meta,
            extra: Default::default(),
            name: map.try_map_name(self.name)?,
        })
    }

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<I>
    where
        M: MapInstr<I, I>,
    {
        TakeOperand {
            _meta: self._meta,
            extra: self.extra,
            name: map.map_name(self.name),
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<I>, Error>
    where
        M: TryMapInstr<I, I, Error>,
    {
        Ok(TakeOperand {
            _meta: self._meta,
            extra: self.extra,
            name: map.try_map_name(self.name)?,
        })
    }
}

impl<I: Instr<Name = super::ResolvedName>> TakeOperand<I> {
    /// Validate physical Operand ownership, independently of displayed names.
    /// Taking an already empty operand remains a checked runtime error.
    pub fn validate_resolved(
        &self,
        layout: &super::StorageLayout,
    ) -> Result<super::OperandLocation, String> {
        validated_compiler_operand_location(&self.name, layout)
    }
}

pub(super) fn validated_compiler_operand_location(
    name: &super::ResolvedName,
    layout: &super::StorageLayout,
) -> Result<super::OperandLocation, String> {
    if let super::NameLocation::Preserved(location) = name.location {
        let slot = layout
            .preserved_slot(location.slot())
            .ok_or("compiler operand has no preserved payload slot")?;
        if !layout.is_expression_temporary(location)
            || slot.storage != super::PreservedSlotStorage::PyObjectOrNull
            || !matches!(slot.init, super::ClosureInit::Deferred)
            || slot.generator_control.is_some()
            || layout.generator_resume_abi.is_none()
            || layout
                .block_parameter_roles
                .iter()
                .any(|role| role.location == name.location)
        {
            return Err("compiler operand requires an explicit suspended object Operand, not cell, source, control or ABI storage".into());
        }
        return Ok(super::OperandLocation::Preserved(location));
    }
    let super::NameLocation::Local(location) = name.location else {
        return Err("compiler operand requires exact local or preserved owning storage".into());
    };
    if layout.stack_slots.get(location.slot() as usize).is_none()
        || !layout.is_expression_temporary(location)
    {
        return Err("compiler operand requires an allocated expression operand".into());
    }
    if let Some(class) = &layout.class_bindings {
        if class.namespace == location {
            return Err("compiler operand cannot consume class namespace".into());
        }
        for slot in &class.slots {
            let owner = slot
                .storage
                .raw_local(layout)
                .ok_or("compiler operand encountered an unresolved class slot")?;
            if owner == location {
                return Err("compiler operand cannot consume a class lexical cell".into());
            }
        }
    }
    for index in 0..layout.cellvars.len() {
        let index = u32::try_from(index).map_err(|_| "too many owned cells")?;
        let owner = super::ClassBindingStorage::Cell(super::CellLocation::Owned(index))
            .raw_local(layout)
            .ok_or("compiler operand encountered an unresolved owned cell")?;
        if owner == location {
            return Err("compiler operand cannot consume an owned lexical cell".into());
        }
    }
    if layout
        .block_parameter_roles
        .iter()
        .any(|role| role.location == super::NameLocation::Local(location))
        || layout.generator_resume_abi.as_ref().is_some_and(|abi| {
            abi.params
                .iter()
                .any(|parameter| layout.generator_resume_local(parameter.role) == Some(location))
        })
    {
        return Err("compiler operand cannot consume an executable control or ABI slot".into());
    }
    Ok(super::OperandLocation::Local(location))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ComprehensionInsertKind {
    ListAppend,
    SetAdd,
    DictSetItem,
}

/// Insert owned operands into a borrowed, live comprehension collection.
/// The container's named Operand slot remains the sole primary owner. The
/// backend checks its exact builtin type and consumes all input references on
/// both success and failure, matching the corresponding native stack operation.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ComprehensionInsert<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub kind: ComprehensionInsertKind,
    pub container: I::Name,
    pub key: Option<Box<I>>,
    pub value: Box<I>,
}

impl<I: Instr> ComprehensionInsert<I> {
    pub fn new(
        kind: ComprehensionInsertKind,
        container: I::Name,
        key: Option<Box<I>>,
        value: Box<I>,
    ) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            kind,
            container,
            key,
            value,
        }
    }

    pub fn with_extra(mut self, extra: I::Extra) -> Self {
        self.extra = extra;
        self
    }
    pub fn extra(&self) -> &I::Extra {
        &self.extra
    }
    pub fn extra_mut(&mut self) -> &mut I::Extra {
        &mut self.extra
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        if (self.kind == ComprehensionInsertKind::DictSetItem) != self.key.is_some() {
            return Err("comprehension insertion requires a key exactly for dict insertion".into());
        }
        Ok(())
    }
}

/// Typed instruction view used by recursive operand-move validation.
pub trait TakeOperandInstruction: Instr + ChildVisitable<Self> {
    fn as_take_operand(&self) -> Option<&TakeOperand<Self>>;
}

struct OperandTakeVisitor<'a, F> {
    on_take: &'a mut F,
}
impl<I, F> Visit<I> for OperandTakeVisitor<'_, F>
where
    I: TakeOperandInstruction<Name = super::ResolvedName>,
    F: FnMut(super::OperandLocation),
{
    fn visit_instr(&mut self, instr: &I) {
        instr.visit_children(self);
        if let Some(location) = instr
            .as_take_operand()
            .and_then(|op| super::OperandLocation::from_name_location(op.name.location))
        {
            (self.on_take)(location);
        }
    }
}

/// Visit consuming reads in child evaluation order, before the enclosing
/// operation. This describes successful evaluation; exceptional cleanup must
/// retain possible prefix owners and inspect their actual nullable slots.
pub fn visit_operand_takes<I>(instr: &I, mut on_take: impl FnMut(super::OperandLocation))
where
    I: TakeOperandInstruction<Name = super::ResolvedName>,
{
    OperandTakeVisitor {
        on_take: &mut on_take,
    }
    .visit_instr(instr);
}

/// Apply the same consuming-read order to the values in a terminator.
pub fn visit_term_operand_takes<I>(
    term: &super::BlockTerm<I>,
    mut on_take: impl FnMut(super::OperandLocation),
) where
    I: TakeOperandInstruction<Name = super::ResolvedName>,
{
    super::walk_term(
        &mut OperandTakeVisitor {
            on_take: &mut on_take,
        },
        term,
    );
}

impl<I: TakeOperandInstruction<Name = super::ResolvedName>> ComprehensionInsert<I> {
    /// This proves only the physical Operand owner, not the runtime container
    /// type. The insertion helper must still enforce the exact builtin kind.
    pub fn validate_resolved(
        &self,
        layout: &super::StorageLayout,
    ) -> Result<super::OperandLocation, String> {
        self.validate_shape()?;
        let container = validated_compiler_operand_location(&self.container, layout)?;
        let takes_container = |instr: &I| {
            instr
                .as_take_operand()
                .is_some_and(|take| take.name.location == container.name_location())
        };
        if super::instr_any(self.value.as_ref(), takes_container)
            || self
                .key
                .as_ref()
                .is_some_and(|key| super::instr_any(key.as_ref(), takes_container))
        {
            return Err("comprehension insertion cannot consume its borrowed container".into());
        }
        Ok(container)
    }
}

impl<I: Instr + PrettyPrint> PrettyPrint for ComprehensionInsert<I> {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        write!(
            printer,
            "comprehension_insert<{:?}>({}, ",
            self.kind,
            self.container.pretty_id()
        )?;
        if let Some(key) = &self.key {
            key.fmt_pretty(printer)?;
            printer.write_str(", ")?;
        }
        self.value.fmt_pretty(printer)?;
        printer.write_str(")")
    }
}
impl<I: Instr> HasMeta for ComprehensionInsert<I> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}
impl<I: Instr> WithMeta for ComprehensionInsert<I> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}
impl<I: Instr + ChildVisitable<I>> ChildVisitable<I> for ComprehensionInsert<I> {
    fn visit_children<V: Visit<I> + ?Sized>(&self, visitor: &mut V) {
        if let Some(key) = &self.key {
            visitor.visit_instr(key);
        }
        visitor.visit_instr(&self.value);
    }
    fn visit_children_mut<V: VisitMut<I> + ?Sized>(&mut self, visitor: &mut V) {
        if let Some(key) = &mut self.key {
            visitor.visit_instr_mut(key);
        }
        visitor.visit_instr_mut(&mut self.value);
    }
}
impl<I: Instr> Mappable<I> for ComprehensionInsert<I> {
    type Mapped<T: Instr> = ComprehensionInsert<T>;
    fn map_children<T: Instr, M: MapInstr<I, T>>(self, map: &mut M) -> Self::Mapped<T> {
        ComprehensionInsert {
            _meta: self._meta,
            extra: Default::default(),
            kind: self.kind,
            container: map.map_name(self.container),
            key: self.key.map(|key| Box::new(map.map_instr(*key))),
            value: Box::new(map.map_instr(*self.value)),
        }
    }
    fn try_map_children<T: Instr, Error, M: TryMapInstr<I, T, Error>>(
        self,
        map: &mut M,
    ) -> Result<Self::Mapped<T>, Error> {
        Ok(ComprehensionInsert {
            _meta: self._meta,
            extra: Default::default(),
            kind: self.kind,
            container: map.try_map_name(self.container)?,
            key: self
                .key
                .map(|key| map.try_map_instr(*key).map(Box::new))
                .transpose()?,
            value: Box::new(map.try_map_instr(*self.value)?),
        })
    }
    fn map_same_children<M: MapInstr<I, I>>(self, map: &mut M) -> Self::Mapped<I> {
        ComprehensionInsert {
            _meta: self._meta,
            extra: self.extra,
            kind: self.kind,
            container: map.map_name(self.container),
            key: self.key.map(|key| Box::new(map.map_instr(*key))),
            value: Box::new(map.map_instr(*self.value)),
        }
    }
    fn try_map_same_children<Error, M: TryMapInstr<I, I, Error>>(
        self,
        map: &mut M,
    ) -> Result<Self::Mapped<I>, Error> {
        Ok(ComprehensionInsert {
            _meta: self._meta,
            extra: self.extra,
            kind: self.kind,
            container: map.try_map_name(self.container)?,
            key: self
                .key
                .map(|key| map.try_map_instr(*key).map(Box::new))
                .transpose()?,
            value: Box::new(map.try_map_instr(*self.value)?),
        })
    }
}

/// The lifetime selected by the producer of a binding. Source locals remain
/// frame roots; operands materialized across expression steps must unwind
/// before a newly raised exception enters its source handler.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum StoreLifetime {
    #[default]
    Frame,
    Operand {
        /// Higher positions unwind first. A producer may reserve a position
        /// before computing the value, as when augmented assignment places
        /// its result below the receiver and key used by the target store.
        unwind_order: u64,
    },
}

/// Why the producer introduced a store, independently of its owning lifetime.
/// Transport copies are resolved before optimization; source assignments never
/// acquire this purpose merely because they copy an exception-valued operand.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum StorePurpose {
    #[default]
    Binding,
    /// An explicit raw local/preserved block-parameter copy. Its value is needed
    /// only when a semantic consumer requires the destination transport.
    BlockParameterTransport,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Store<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
    pub value: Box<I>,
    pub lifetime: StoreLifetime,
    pub purpose: StorePurpose,
}

impl<I> PrettyPrint for Store<I>
where
    I: Instr + PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        if self.name.pretty_id() == self.name.id_str() {
            write!(printer, "StoreName({:?}, ", self.name.id_str())?;
        } else {
            write!(printer, "StoreLocation({}, ", self.name.pretty_id())?;
        }
        self.value.as_ref().fmt_pretty(printer)?;
        printer.write_char(')')
    }
}

impl<I: Instr> Store<I> {
    pub fn new(name: impl Into<I::Name>, value: impl Into<Box<I>>) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            name: name.into(),
            value: value.into(),
            lifetime: StoreLifetime::Frame,
            purpose: StorePurpose::Binding,
        }
    }

    pub fn with_lifetime(mut self, lifetime: StoreLifetime) -> Self {
        self.lifetime = lifetime;
        self
    }

    pub fn with_purpose(mut self, purpose: StorePurpose) -> Self {
        self.purpose = purpose;
        self
    }

    pub fn with_extra(mut self, extra: I::Extra) -> Self {
        self.extra = extra;
        self
    }

    pub fn extra(&self) -> &I::Extra {
        &self.extra
    }

    pub fn extra_mut(&mut self) -> &mut I::Extra {
        &mut self.extra
    }
}

impl<I: Instr> HasMeta for Store<I> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<I: Instr> WithMeta for Store<I> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<I> ChildVisitable<I> for Store<I>
where
    I: Instr + ChildVisitable<I>,
{
    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: crate::block_py::VisitMut<I> + ?Sized,
    {
        visitor.visit_instr_mut(&mut self.value);
    }

    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: crate::block_py::Visit<I> + ?Sized,
    {
        visitor.visit_instr(&self.value);
    }
}

impl<I: Instr> Mappable<I> for Store<I> {
    type Mapped<T: Instr> = Store<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<I, T>,
    {
        Store {
            _meta: self._meta,
            extra: Default::default(),
            name: map.map_name(self.name),
            value: Box::new(map.map_instr(*self.value)),
            lifetime: self.lifetime,
            purpose: self.purpose,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<I, T, Error>,
    {
        Ok(Store {
            _meta: self._meta,
            extra: Default::default(),
            name: map.try_map_name(self.name)?,
            value: Box::new(map.try_map_instr(*self.value)?),
            lifetime: self.lifetime,
            purpose: self.purpose,
        })
    }

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<I>
    where
        M: MapInstr<I, I>,
    {
        Store {
            _meta: self._meta,
            extra: self.extra,
            name: map.map_name(self.name),
            value: Box::new(map.map_instr(*self.value)),
            lifetime: self.lifetime,
            purpose: self.purpose,
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<I>, Error>
    where
        M: TryMapInstr<I, I, Error>,
    {
        Ok(Store {
            _meta: self._meta,
            extra: self.extra,
            name: map.try_map_name(self.name)?,
            value: Box::new(map.try_map_instr(*self.value)?),
            lifetime: self.lifetime,
            purpose: self.purpose,
        })
    }
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Del<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
    pub quietly: bool,
}

impl<I: Instr> PrettyPrint for Del<I> {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        write!(
            printer,
            "Del {{ name: {:?}, quietly: {} }}",
            self.name.pretty_id(),
            self.quietly
        )
    }
}

impl<I: Instr> Del<I> {
    pub fn new(name: impl Into<I::Name>, quietly: bool) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            name: name.into(),
            quietly,
        }
    }

    pub fn with_extra(mut self, extra: I::Extra) -> Self {
        self.extra = extra;
        self
    }

    pub fn extra(&self) -> &I::Extra {
        &self.extra
    }

    pub fn extra_mut(&mut self) -> &mut I::Extra {
        &mut self.extra
    }
}

impl<I: Instr> HasMeta for Del<I> {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl<I: Instr> WithMeta for Del<I> {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<I> ChildVisitable<I> for Del<I>
where
    I: Instr + ChildVisitable<I>,
{
    fn visit_children<V>(&self, _visitor: &mut V)
    where
        V: crate::block_py::Visit<I> + ?Sized,
    {
    }

    fn visit_children_mut<V>(&mut self, _visitor: &mut V)
    where
        V: crate::block_py::VisitMut<I> + ?Sized,
    {
    }
}

impl<I: Instr> Mappable<I> for Del<I> {
    type Mapped<T: Instr> = Del<T>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<I, T>,
    {
        Del {
            _meta: self._meta,
            extra: Default::default(),
            name: map.map_name(self.name),
            quietly: self.quietly,
        }
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<I, T, Error>,
    {
        Ok(Del {
            _meta: self._meta,
            extra: Default::default(),
            name: map.try_map_name(self.name)?,
            quietly: self.quietly,
        })
    }

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<I>
    where
        M: MapInstr<I, I>,
    {
        Del {
            _meta: self._meta,
            extra: self.extra,
            name: map.map_name(self.name),
            quietly: self.quietly,
        }
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<I>, Error>
    where
        M: TryMapInstr<I, I, Error>,
    {
        Ok(Del {
            _meta: self._meta,
            extra: self.extra,
            name: map.try_map_name(self.name)?,
            quietly: self.quietly,
        })
    }
}

define_instr! {
    /// CPython's namespace-only annotation initialization. Name binding selects
    /// the actual class mapping; None explicitly denotes the module globals.
    /// Codegen must not consult a Python frame or a mutable locals() binding.
    pub struct SetupAnnotations<E> {
        namespace: Option<Box<E>>,
    }
}

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum TypeParameterKind {
    TypeVar,
    TypeVarBound,
    TypeVarConstraints,
    ParamSpec,
    TypeVarTuple,
}

define_instr! {
    /// Evaluate defaults in the enclosing scope before creating and calling
    /// the actual generic-parameter function. The function operand is a
    /// compiler-selected MakeFunction child, never a helper-name lookup.
    pub struct ConstructTypeParameterScope<E> {
        definition: SourceIdentity,
        scope_function_id: RuntimeFunctionId,
        positional_defaults: Option<Box<E>>,
        keyword_defaults: Option<Box<E>>,
        scope_function: Box<E>,
    }
}

define_instr! {
    /// Construct the native implicit Generic base before explicit base and
    /// keyword expressions. This operation grants no class capability.
    pub struct SubscriptGeneric<E> {
        definition: SourceIdentity,
        type_parameters: Box<E>,
    }
}

define_instr! {
    /// Finish generic function metadata before decorators and completion.
    pub struct SetFunctionTypeParameters<E> {
        definition: SourceIdentity,
        function_id: RuntimeFunctionId,
        function: Box<E>,
        type_parameters: Box<E>,
    }
}

impl<E: Instr> ConstructTypeParameterScope<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        self.positional_defaults
            .as_deref()
            .into_iter()
            .chain(self.keyword_defaults.as_deref())
            .chain(std::iter::once(self.scope_function.as_ref()))
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        self.positional_defaults
            .as_deref_mut()
            .into_iter()
            .chain(self.keyword_defaults.as_deref_mut())
            .chain(std::iter::once(self.scope_function.as_mut()))
    }
}

impl<E: Instr> SetFunctionTypeParameters<E> {
    pub fn operands(&self) -> [&E; 2] {
        [&self.function, &self.type_parameters]
    }

    pub fn operands_mut(&mut self) -> [&mut E; 2] {
        [&mut self.function, &mut self.type_parameters]
    }
}

impl<E: Instr> SubscriptGeneric<E> {
    pub fn operands(&self) -> [&E; 1] {
        [&self.type_parameters]
    }
    pub fn operands_mut(&mut self) -> [&mut E; 1] {
        [&mut self.type_parameters]
    }
}

define_instr! {
    /// Create the actual native alias from a source-matched lazy evaluator.
    /// The factory does not execute its value or confer an immutable capability.
    pub struct CreateTypeAlias<E> {
        definition: SourceIdentity,
        evaluator_function: RuntimeFunctionId,
        name: Box<E>,
        type_parameters: Box<E>,
        evaluator: Box<E>,
    }
}

define_instr! {
    pub struct CreateTypeParameter<E> {
        definition: SourceIdentity,
        kind: TypeParameterKind,
        name: Box<E>,
        evaluator_function: Option<RuntimeFunctionId>,
        evaluator: Option<Box<E>>,
    }
}

define_instr! {
    /// Attach a separately created default evaluator at its original execution
    /// point, after parameter creation and before publication in the tuple.
    pub struct SetTypeParameterDefault<E> {
        definition: SourceIdentity,
        evaluator_function: RuntimeFunctionId,
        parameter: Box<E>,
        evaluator: Box<E>,
    }
}

impl<E: Instr> CreateTypeAlias<E> {
    pub fn operands(&self) -> [&E; 3] {
        [&self.name, &self.type_parameters, &self.evaluator]
    }

    pub fn operands_mut(&mut self) -> [&mut E; 3] {
        [
            &mut self.name,
            &mut self.type_parameters,
            &mut self.evaluator,
        ]
    }
}

impl<E: Instr> CreateTypeParameter<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        std::iter::once(self.name.as_ref()).chain(self.evaluator.as_deref())
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        std::iter::once(self.name.as_mut()).chain(self.evaluator.as_deref_mut())
    }
}

impl<E: Instr> SetTypeParameterDefault<E> {
    pub fn operands(&self) -> [&E; 2] {
        [&self.parameter, &self.evaluator]
    }

    pub fn operands_mut(&mut self) -> [&mut E; 2] {
        [&mut self.parameter, &mut self.evaluator]
    }
}

define_instr! {
    /// Native annotation format comparison followed by the canonical
    /// NotImplementedError, independent of mutable Python exception bindings.
    pub struct CheckAnnotationFormat<E> {
        format: Box<E>,
    }
}

define_instr! {
    /// Allocate the exact set used by CPython's conditional-annotation protocol.
    /// This is compiler-owned state creation, not a call through Python's set
    /// binding or a mutable helper attribute.
    pub struct NewAnnotationSet<E> {}
}

define_instr! {
    /// Mark one annotation statement only after its ordinary assignment has
    /// completed. The source planner chooses the native provider index.
    pub struct RecordAnnotation<E> {
        indices: Box<E>,
        index: u32,
    }
}

define_instr! {
    pub struct MakeCell<E> {
        initial_value: Option<Box<E>>,
    }
}

impl<E: Instr> MakeCell<E> {
    pub fn empty() -> Self {
        Self::new(None)
    }

    pub fn with_initial_value(initial_value: E) -> Self {
        Self::new(Some(Box::new(initial_value)))
    }
}

define_instr! {
    pub struct CellRefForName {
        logical_name: String,
        lexical_scope: Option<soac_contracts::SourceIdentity>,
    }
}

define_instr! {
    pub struct CellRef {
        location: CellLocation,
    }
}

define_instr! {
    pub struct MakeFunction<E> {
        function_id: RuntimeFunctionId,
        kind: FunctionKind,
        param_defaults: Box<E>,
        annotate_fn: Box<E>,
        class_namespace: Option<Box<E>>,
        creation_cells: Vec<E>,
    }
}

impl<E: Instr> MakeFunction<E> {
    pub fn function_id(&self) -> RuntimeFunctionId {
        self.function_id
    }

    pub fn set_function_id(&mut self, function_id: RuntimeFunctionId) {
        self.function_id = function_id;
    }
}

define_instr! {
    pub struct MakeFunctionWithClosure<E> {
        function_id: RuntimeFunctionId,
        kind: FunctionKind,
        captures: Box<E>,
        param_defaults: Box<E>,
        annotate_fn: Box<E>,
        class_namespace: Option<Box<E>>,
        creation_cells: Vec<E>,
    }
}

define_instr! {
    /// Release the exact created helper's ephemeral construction cells on
    /// every exit from its class-statement region. This is idempotent resource
    /// cleanup, never a revocation of a finalized class or function contract.
    pub struct DiscardClassConstructionCaptures<E> {
        function: Box<E>,
    }
}

impl<E: Instr> DiscardClassConstructionCaptures<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        std::iter::once(self.function.as_ref())
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        std::iter::once(self.function.as_mut())
    }
}

define_instr! {
    /// Apply one source-selected builtin descriptor decorator to the actual
    /// function created at this definition site. The decorator is evaluated
    /// first, before defaults and function creation. Runtime identity and
    /// namespace-execution checks decide whether native birth is available;
    /// a rebound decorator still receives the ordinary call exactly once.
    pub struct ApplyFunctionDescriptor<E> {
        definition: soac_contracts::SourceIdentity,
        function_id: RuntimeFunctionId,
        decorator: Box<E>,
        function: Box<E>,
        frame_namespace: Option<FrameNamespace<E>>,
    }
}

impl<E: Instr> ApplyFunctionDescriptor<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        [self.decorator.as_ref(), self.function.as_ref()]
            .into_iter()
            .chain(
                self.frame_namespace
                    .as_ref()
                    .and_then(FrameNamespace::mapping),
            )
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        [self.decorator.as_mut(), self.function.as_mut()]
            .into_iter()
            .chain(
                self.frame_namespace
                    .as_mut()
                    .and_then(FrameNamespace::mapping_mut),
            )
    }
}

define_instr! {
    /// Complete one compiler-created undecorated source function definition
    /// after metadata setup and before its source binding. This is an adoption
    /// boundary, not a static callable capability or permission to skip checks.
    pub struct CompleteFunctionDefinition<E> {
        definition: SourceIdentity,
        function_id: RuntimeFunctionId,
        function: Box<E>,
    }
}

define_instr! {
    /// An authenticated lexical class-construction site, not a class capability.
    /// The runtime validates its actual namespace function and execution owner
    /// before preparing any native construction handle. Unrecognized decorator
    /// applications remain ordinary calls outside this operation; such classes
    /// must decline irreversible participation before native type allocation.
    pub struct ConstructClass<E> {
        definition: SourceIdentity,
        construction_function: RuntimeFunctionId,
        name: Box<E>,
        namespace_function: Box<E>,
        bases: Box<E>,
        keywords: Box<E>,
        requires_class_cell: Box<E>,
        requires_class_dict_cell: Box<E>,
        first_line: Box<E>,
        decorator_preparation: Option<Box<E>>,
    }
}

impl<E: Instr> ConstructClass<E> {
    pub fn operands(&self) -> impl Iterator<Item = &E> {
        [
            self.name.as_ref(),
            self.namespace_function.as_ref(),
            self.bases.as_ref(),
            self.keywords.as_ref(),
            self.requires_class_cell.as_ref(),
            self.requires_class_dict_cell.as_ref(),
            self.first_line.as_ref(),
        ]
        .into_iter()
        .chain(self.decorator_preparation.as_deref())
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        [
            self.name.as_mut(),
            self.namespace_function.as_mut(),
            self.bases.as_mut(),
            self.keywords.as_mut(),
            self.requires_class_cell.as_mut(),
            self.requires_class_dict_cell.as_mut(),
            self.first_line.as_mut(),
        ]
        .into_iter()
        .chain(self.decorator_preparation.as_deref_mut())
    }
}

impl<E: Instr> MakeFunctionWithClosure<E> {
    pub fn function_id(&self) -> RuntimeFunctionId {
        self.function_id
    }

    pub fn set_function_id(&mut self, function_id: RuntimeFunctionId) {
        self.function_id = function_id;
    }

    pub fn operands(&self) -> impl Iterator<Item = &E> {
        [
            self.captures.as_ref(),
            self.param_defaults.as_ref(),
            self.annotate_fn.as_ref(),
        ]
        .into_iter()
        .chain(self.class_namespace.as_deref())
        .chain(self.creation_cells.iter())
    }

    pub fn operands_mut(&mut self) -> impl Iterator<Item = &mut E> {
        [
            self.captures.as_mut(),
            self.param_defaults.as_mut(),
            self.annotate_fn.as_mut(),
        ]
        .into_iter()
        .chain(self.class_namespace.as_deref_mut())
        .chain(self.creation_cells.iter_mut())
    }
}

macro_rules! define_suspend_instr {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
        pub struct $name<E: Instr> {
            _meta: Meta,
            pub extra: E::Extra,
            pub value: Box<E>,
        }

        impl<E> PrettyPrint for $name<E>
        where
            E: Instr + PrettyPrint,
        {
            fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
                printer.write_str($prefix)?;
                self.value.fmt_pretty(printer)
            }
        }

        impl<E: Instr> $name<E> {
            pub fn new(value: impl Into<Box<E>>) -> Self {
                Self {
                    _meta: Meta::default(),
                    extra: Default::default(),
                    value: value.into(),
                }
            }

            pub fn with_extra(mut self, extra: E::Extra) -> Self {
                self.extra = extra;
                self
            }

            pub fn extra(&self) -> &E::Extra {
                &self.extra
            }

            pub fn extra_mut(&mut self) -> &mut E::Extra {
                &mut self.extra
            }
        }

        impl<E: Instr> HasMeta for $name<E> {
            fn meta(&self) -> Meta {
                self._meta.clone()
            }
        }

        impl<E: Instr> WithMeta for $name<E> {
            fn with_meta(mut self, meta: Meta) -> Self {
                self._meta = meta;
                self
            }
        }

        impl<E> ChildVisitable<E> for $name<E>
        where
            E: Instr + ChildVisitable<E>,
        {
            fn visit_children<V>(&self, visitor: &mut V)
            where
                V: Visit<E> + ?Sized,
            {
                InstrField::<E>::visit_field(&self.value, visitor);
            }

            fn visit_children_mut<V>(&mut self, visitor: &mut V)
            where
                V: VisitMut<E> + ?Sized,
            {
                InstrField::<E>::visit_field_mut(&mut self.value, visitor);
            }
        }

        impl<E: Instr> Mappable<E> for $name<E> {
            type Mapped<T: Instr> = $name<T>;

            fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
            where
                T: Instr,
                M: MapInstr<E, T>,
            {
                $name::<T> {
                    _meta: self._meta,
                    extra: Default::default(),
                    value: InstrField::<E>::map_field::<T, M>(self.value, map),
                }
            }

            fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
            where
                T: Instr,
                M: TryMapInstr<E, T, Error>,
            {
                Ok($name::<T> {
                    _meta: self._meta,
                    extra: Default::default(),
                    value: InstrField::<E>::try_map_field::<T, Error, M>(self.value, map)?,
                })
            }

            fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
            where
                M: MapInstr<E, E>,
            {
                $name::<E> {
                    _meta: self._meta,
                    extra: self.extra,
                    value: InstrField::<E>::map_field::<E, M>(self.value, map),
                }
            }

            fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
            where
                M: TryMapInstr<E, E, Error>,
            {
                Ok($name::<E> {
                    _meta: self._meta,
                    extra: self.extra,
                    value: InstrField::<E>::try_map_field::<E, Error, M>(self.value, map)?,
                })
            }
        }
    };
}

define_suspend_instr!(Await, "await ");
define_suspend_instr!(Yield, "yield ");
define_suspend_instr!(YieldFrom, "yield from ");

define_ruff_instr! {
    pub struct ExprBoolOp<E> {
        op: ast::BoolOp,
        values: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct ExprNamed<E> {
        target: Box<E>,
        value: Box<E>,
    }
}

define_ruff_instr! {
    pub struct ExprLambda<E> {
        parameters: Option<Box<ast::Parameters>>,
        body: Box<E>,
    }
}

define_ruff_instr! {
    pub struct ExprIf<E> {
        test: Box<E>,
        body: Box<E>,
        orelse: Box<E>,
    }
}

/// One source dictionary item. A missing key is a mapping unpack.
/// Children stay in the bridge IR so compiler-owned operand moves never
/// round-trip through a source AST name.
#[derive(Clone, Debug)]
pub struct ExprDictItem<E: Instr> {
    pub key: Option<E>,
    pub value: E,
}

impl<E: Instr> InstrField<E> for ExprDictItem<E> {
    type Mapped<T: Instr> = ExprDictItem<T>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        E: ChildVisitable<E>,
        V: Visit<E> + ?Sized,
    {
        if let Some(key) = &self.key {
            visitor.visit_instr(key);
        }
        visitor.visit_instr(&self.value);
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        E: ChildVisitable<E>,
        V: VisitMut<E> + ?Sized,
    {
        if let Some(key) = &mut self.key {
            visitor.visit_instr_mut(key);
        }
        visitor.visit_instr_mut(&mut self.value);
    }

    fn map_field<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        ExprDictItem {
            key: self.key.map(|key| map.map_instr(key)),
            value: map.map_instr(self.value),
        }
    }

    fn try_map_field<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        Ok(ExprDictItem {
            key: self.key.map(|key| map.try_map_instr(key)).transpose()?,
            value: map.try_map_instr(self.value)?,
        })
    }
}

impl<E: Instr + PrettyPrint> PrettyPrint for ExprDictItem<E> {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        if let Some(key) = &self.key {
            key.fmt_pretty(printer)?;
            printer.write_str(": ")?;
        } else {
            printer.write_str("**")?;
        }
        self.value.fmt_pretty(printer)
    }
}

define_ruff_instr! {
    pub struct ExprDict<E> {
        items: Vec<ExprDictItem<E>>,
    }
}

define_ruff_instr! {
    pub struct ExprSet<E> {
        elts: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct ExprListComp<E> {
        elt: Box<E>,
        generators: Vec<ast::Comprehension>,
    }
}

define_ruff_instr! {
    pub struct ExprSetComp<E> {
        elt: Box<E>,
        generators: Vec<ast::Comprehension>,
    }
}

define_ruff_instr! {
    pub struct ExprDictComp<E> {
        key: Box<E>,
        value: Box<E>,
        generators: Vec<ast::Comprehension>,
    }
}

define_ruff_instr! {
    pub struct ExprGenerator<E> {
        elt: Box<E>,
        generators: Vec<ast::Comprehension>,
        parenthesized: bool,
    }
}

define_ruff_instr! {
    pub struct ExprCompare<E> {
        left: Box<E>,
        ops: Vec<ast::CmpOp>,
        comparators: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct ExprFString {
        value: ast::FStringValue,
    }
}

define_ruff_instr! {
    pub struct ExprTString {
        value: ast::TStringValue,
    }
}

define_ruff_instr! {
    pub struct ExprStringLiteral {
        value: ast::StringLiteralValue,
    }
}

define_ruff_instr! {
    pub struct ExprBytesLiteral {
        value: ast::BytesLiteralValue,
    }
}

define_ruff_instr! {
    pub struct ExprNumberLiteral {
        value: ast::Number,
    }
}

define_ruff_instr! {
    pub struct ExprBooleanLiteral {
        value: bool,
    }
}

define_ruff_instr! {
    pub struct ExprNoneLiteral {
    }
}

define_ruff_instr! {
    pub struct ExprEllipsisLiteral {
    }
}

define_ruff_instr! {
    pub struct ExprAttribute<E> {
        value: Box<E>,
        attr: ast::Identifier,
        ctx: ast::ExprContext,
    }
}

define_ruff_instr! {
    pub struct ExprSubscript<E> {
        value: Box<E>,
        slice: Box<E>,
        ctx: ast::ExprContext,
    }
}

define_ruff_instr! {
    pub struct ExprStarred<E> {
        value: Box<E>,
        ctx: ast::ExprContext,
    }
}

define_ruff_instr! {
    pub struct ExprName {
        id: ast::name::Name,
        ctx: ast::ExprContext,
    }
}

define_ruff_instr! {
    pub struct ExprList<E> {
        elts: Vec<E>,
        ctx: ast::ExprContext,
    }
}

define_ruff_instr! {
    pub struct ExprTuple<E> {
        elts: Vec<E>,
        ctx: ast::ExprContext,
        parenthesized: bool,
    }
}

define_ruff_instr! {
    pub struct ExprSlice<E> {
        lower: Option<Box<E>>,
        upper: Option<Box<E>>,
        step: Option<Box<E>>,
    }
}

define_ruff_instr! {
    pub struct ExprIpyEscapeCommand {
        kind: ast::IpyEscapeKind,
        value: Box<str>,
    }
}

define_ruff_instr! {
    pub struct StmtFunctionDef<E> {
        is_async: bool,
        decorator_list: Vec<ast::Decorator>,
        name: ast::Identifier,
        type_params: Option<Box<ast::TypeParams>>,
        parameters: Box<ast::Parameters>,
        returns: Option<Box<E>>,
        body: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtClassDef<E> {
        decorator_list: Vec<ast::Decorator>,
        name: ast::Identifier,
        type_params: Option<Box<ast::TypeParams>>,
        arguments: Option<Box<ast::Arguments>>,
        body: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtReturn<E> {
        value: Box<E>,
    }
}

define_ruff_instr! {
    pub struct StmtDelete<E> {
        targets: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtTypeAlias<E> {
        name: Box<E>,
        type_params: Option<Box<ast::TypeParams>>,
        value: Box<E>,
    }
}

define_ruff_instr! {
    pub struct StmtAssign<E> {
        targets: Vec<E>,
        value: Box<E>,
    }
}

define_ruff_instr! {
    pub struct StmtAugAssign<E> {
        target: Box<E>,
        op: ast::Operator,
        value: Box<E>,
    }
}

define_ruff_instr! {
    pub struct StmtAnnAssign<E> {
        target: Box<E>,
        annotation: Box<E>,
        value: Option<Box<E>>,
        simple: bool,
    }
}

define_ruff_instr! {
    pub struct StmtFor<E> {
        is_async: bool,
        target: Box<E>,
        iter: Box<E>,
        body: Vec<E>,
        orelse: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtWhile<E> {
        test: Box<E>,
        body: Vec<E>,
        orelse: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtIf<E> {
        test: Box<E>,
        body: Vec<E>,
        orelse: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtWith<E> {
        is_async: bool,
        items: Vec<ast::WithItem>,
        body: Vec<E>,
    }
}

define_ruff_instr! {
    pub struct StmtMatch<E> {
        subject: Box<E>,
        cases: Vec<ast::MatchCase>,
    }
}

define_ruff_instr! {
    pub struct StmtRaise<E> {
        exc: Option<Box<E>>,
        cause: Option<Box<E>>,
    }
}

define_ruff_instr! {
    pub struct StmtTry<E> {
        body: Vec<E>,
        handlers: Vec<ast::ExceptHandler>,
        orelse: Vec<E>,
        finalbody: Vec<E>,
        is_star: bool,
    }
}

define_ruff_instr! {
    pub struct StmtAssert<E> {
        test: Box<E>,
        msg: Option<Box<E>>,
    }
}

define_ruff_instr! {
    pub struct StmtImport {
        names: Vec<ast::Alias>,
    }
}

define_ruff_instr! {
    pub struct StmtImportFrom {
        module: Option<ast::Identifier>,
        names: Vec<ast::Alias>,
        level: u32,
    }
}

define_ruff_instr! {
    pub struct StmtGlobal {
        names: Vec<ast::Identifier>,
    }
}

define_ruff_instr! {
    pub struct StmtNonlocal {
        names: Vec<ast::Identifier>,
    }
}

define_ruff_instr! {
    pub struct StmtExpr<E> {
        value: Box<E>,
    }
}

define_ruff_instr! {
    pub struct StmtPass {
    }
}

define_ruff_instr! {
    pub struct StmtBreak {
    }
}

define_ruff_instr! {
    pub struct StmtContinue {
    }
}

define_ruff_instr! {
    pub struct StmtIpyEscapeCommand {
        kind: ast::IpyEscapeKind,
        value: Box<str>,
    }
}

#[cfg(test)]
mod tests {
    use super::super::UnresolvedName;
    use super::super::{Visit, VisitMut};
    use super::*;

    #[derive(Clone, Debug)]
    enum TestInstr {}

    impl Instr for TestInstr {
        type Name = UnresolvedName;
        type Extra = &'static str;
    }

    #[derive(Clone, Debug)]
    enum OtherInstr {}

    impl Instr for OtherInstr {
        type Name = UnresolvedName;
        type Extra = &'static str;
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ChildInstr(&'static str);

    impl Instr for ChildInstr {
        type Name = UnresolvedName;
        type Extra = &'static str;
    }

    impl PrettyPrint for ChildInstr {
        fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
            write!(printer, "<{}>", self.0)
        }
    }

    impl ChildVisitable<ChildInstr> for ChildInstr {
        fn visit_children<V>(&self, _visitor: &mut V)
        where
            V: Visit<ChildInstr> + ?Sized,
        {
        }

        fn visit_children_mut<V>(&mut self, _visitor: &mut V)
        where
            V: VisitMut<ChildInstr> + ?Sized,
        {
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MappedChildInstr(&'static str);

    impl Instr for MappedChildInstr {
        type Name = UnresolvedName;
        type Extra = &'static str;
    }

    define_instr! {
        #[allow(dead_code)]
        struct FieldRichOperation<E> {
            name: E::Name,
            args: Vec<CallArgPositional<E>>,
            keywords: Vec<CallArgKeyword<E>>,
            fallback: Option<Box<E>>,
            values: Vec<E>,
        }
    }

    struct TestToOther;

    impl MapInstr<TestInstr, OtherInstr> for TestToOther {
        fn map_instr(&mut self, instr: TestInstr) -> OtherInstr {
            match instr {}
        }

        fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
            name
        }
    }

    #[test]
    fn same_child_mapping_preserves_handwritten_operation_extra() {
        let op = Load::<TestInstr>::new("x").with_extra("borrowed");
        let mapped = op.map_same_children(&mut |instr| instr);

        assert_eq!(*mapped.extra(), "borrowed");
    }

    #[test]
    fn same_child_mapping_preserves_macro_operation_extra() {
        let op = Tuple::<TestInstr>::new(Vec::new()).with_extra("effect-only");
        let mapped = op.map_same_children(&mut |instr| instr);

        assert_eq!(*mapped.extra(), "effect-only");
    }

    #[test]
    fn cross_instr_mapping_uses_destination_default_extra() {
        let op = Load::<TestInstr>::new("x").with_extra("borrowed");
        let mapped: Load<OtherInstr> = op.map_children(&mut TestToOther);

        assert_eq!(*mapped.extra(), "");
    }

    #[test]
    fn store_mapping_preserves_transport_purpose_independently_of_lifetime() {
        struct AcrossKinds;
        impl MapInstr<ChildInstr, MappedChildInstr> for AcrossKinds {
            fn map_instr(&mut self, value: ChildInstr) -> MappedChildInstr {
                MappedChildInstr(value.0)
            }

            fn map_name(&mut self, _: UnresolvedName) -> UnresolvedName {
                "renamed_destination".into()
            }
        }
        impl TryMapInstr<ChildInstr, MappedChildInstr, ()> for AcrossKinds {
            fn try_map_instr(&mut self, value: ChildInstr) -> Result<MappedChildInstr, ()> {
                Ok(MappedChildInstr(value.0))
            }

            fn try_map_name(&mut self, _: UnresolvedName) -> Result<UnresolvedName, ()> {
                Ok("renamed_destination".into())
            }
        }
        for (lifetime, purpose) in [
            StoreLifetime::Frame,
            StoreLifetime::Operand { unwind_order: 17 },
        ]
        .into_iter()
        .flat_map(|lifetime| {
            [StorePurpose::BlockParameterTransport]
                .into_iter()
                .map(move |purpose| (lifetime, purpose))
        }) {
            let original = Store::<ChildInstr>::new("original_destination", ChildInstr("value"))
                .with_lifetime(lifetime)
                .with_purpose(purpose);
            let same = [
                original.clone().map_same_children(&mut |value| value),
                original
                    .clone()
                    .try_map_same_children(&mut |value| Ok::<_, ()>(value))
                    .unwrap(),
            ];
            for mapped in same {
                assert_eq!(mapped.purpose, purpose);
                assert_eq!(mapped.lifetime, lifetime);
                assert_eq!(*mapped.value, ChildInstr("value"));
            }
            let across = [
                original.clone().map_children(&mut AcrossKinds),
                original.try_map_children(&mut AcrossKinds).unwrap(),
            ];
            for mapped in across {
                assert_eq!(mapped.purpose, purpose);
                assert_eq!(mapped.lifetime, lifetime);
                assert_eq!(mapped.name.id_str(), "renamed_destination");
                assert_eq!(*mapped.value, MappedChildInstr("value"));
            }
        }
    }

    #[test]
    fn macro_operation_maps_name_and_call_arg_fields() {
        struct ChildToMapped;

        impl MapInstr<ChildInstr, MappedChildInstr> for ChildToMapped {
            fn map_instr(&mut self, instr: ChildInstr) -> MappedChildInstr {
                MappedChildInstr(instr.0)
            }

            fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
                format!("mapped_{}", name.id_str()).into()
            }
        }

        let mut op = FieldRichOperation::<ChildInstr>::new(
            "source",
            vec![
                CallArgPositional::Positional(ChildInstr("arg0")),
                CallArgPositional::Starred(ChildInstr("starred")),
            ],
            vec![
                CallArgKeyword::Named {
                    arg: "kw".into(),
                    value: ChildInstr("kwarg"),
                },
                CallArgKeyword::Starred(ChildInstr("kwargs")),
            ],
            Some(Box::new(ChildInstr("fallback"))),
            vec![ChildInstr("value")],
        )
        .with_extra("source-extra");

        assert_eq!(*op.extra(), "source-extra");
        *op.extra_mut() = "updated-extra";
        assert_eq!(*op.extra(), "updated-extra");

        let mapped: FieldRichOperation<MappedChildInstr> = op.map_children(&mut ChildToMapped);

        assert_eq!(mapped.name.id_str(), "mapped_source");
        assert_eq!(*mapped.extra(), "");
        assert!(matches!(
            mapped.args.as_slice(),
            [
                CallArgPositional::Positional(MappedChildInstr("arg0")),
                CallArgPositional::Starred(MappedChildInstr("starred")),
            ]
        ));
        assert!(matches!(
            mapped.keywords.as_slice(),
            [
                CallArgKeyword::Named { arg, value: MappedChildInstr("kwarg") },
                CallArgKeyword::Starred(MappedChildInstr("kwargs")),
            ] if arg.as_str() == "kw"
        ));
        assert_eq!(
            mapped.fallback.as_deref(),
            Some(&MappedChildInstr("fallback"))
        );
        assert_eq!(mapped.values, vec![MappedChildInstr("value")]);
    }

    #[test]
    fn macro_operation_visits_call_arg_fields() {
        struct Counter(usize);

        impl Visit<ChildInstr> for Counter {
            fn visit_instr(&mut self, _expr: &ChildInstr) {
                self.0 += 1;
            }
        }

        let op = FieldRichOperation::<ChildInstr>::new(
            "source",
            vec![
                CallArgPositional::Positional(ChildInstr("arg0")),
                CallArgPositional::Starred(ChildInstr("starred")),
            ],
            vec![
                CallArgKeyword::Named {
                    arg: "kw".into(),
                    value: ChildInstr("kwarg"),
                },
                CallArgKeyword::Starred(ChildInstr("kwargs")),
            ],
            Some(Box::new(ChildInstr("fallback"))),
            vec![ChildInstr("value")],
        );

        let mut counter = Counter(0);
        op.visit_children(&mut counter);

        assert_eq!(counter.0, 6);
    }

    #[test]
    fn macro_operation_pretty_prints_child_fields_without_child_debug() {
        let op = FieldRichOperation::<ChildInstr>::new(
            "source",
            vec![
                CallArgPositional::Positional(ChildInstr("arg0")),
                CallArgPositional::Starred(ChildInstr("starred")),
            ],
            vec![
                CallArgKeyword::Named {
                    arg: "kw".into(),
                    value: ChildInstr("kwarg"),
                },
                CallArgKeyword::Starred(ChildInstr("kwargs")),
            ],
            Some(Box::new(ChildInstr("fallback"))),
            vec![ChildInstr("value")],
        );

        let rendered = op.pretty_print();

        assert_eq!(
            rendered,
            "FieldRichOperation(source, [Positional(<arg0>), Starred(<starred>)], [Named { arg: KeywordName { id: \"kw\" }, value: <kwarg> }, Starred(<kwargs>)], Some(<fallback>), [<value>])"
        );
        assert!(!rendered.contains("ChildInstr"));
    }
    #[derive(Clone, Debug)]
    enum OperandTestExpr {
        Leaf,
        Take(TakeOperand<Self>),
        Sequence(Vec<Self>),
    }
    impl Instr for OperandTestExpr {
        type Name = super::super::ResolvedName;
        type Extra = ();
    }
    impl ChildVisitable<Self> for OperandTestExpr {
        fn visit_children<V: Visit<Self> + ?Sized>(&self, visitor: &mut V) {
            if let Self::Sequence(values) = self {
                for value in values {
                    visitor.visit_instr(value);
                }
            }
        }
        fn visit_children_mut<V: VisitMut<Self> + ?Sized>(&mut self, visitor: &mut V) {
            if let Self::Sequence(values) = self {
                for value in values {
                    visitor.visit_instr_mut(value);
                }
            }
        }
    }
    impl TakeOperandInstruction for OperandTestExpr {
        fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
            match self {
                Self::Take(op) => Some(op),
                _ => None,
            }
        }
    }
    fn operand_test_name(name: &str, index: u32) -> super::super::ResolvedName {
        super::super::ResolvedName {
            id: name.into(),
            location: super::super::NameLocation::Local(super::super::LocalLocation(index)),
        }
    }
    fn operand_test_layout() -> super::super::StorageLayout {
        use super::super::{LocalLocation, StorageLayout};
        StorageLayout {
            stack_slots: vec!["unused".into(), "operand".into(), "namespace".into()],
            expression_temporaries: vec![LocalLocation(1).into()],
            ..StorageLayout::default()
        }
    }

    #[test]
    fn operand_take_rejects_protected_physical_owner_aliases() {
        use super::super::*;
        let take =
            TakeOperand::<OperandTestExpr>::new(operand_test_name("not_the_storage_name", 1));
        let layout = operand_test_layout();
        assert_eq!(
            take.validate_resolved(&layout).unwrap(),
            OperandLocation::Local(LocalLocation(1))
        );
        let mut invalid = Vec::new();
        let code = NativeCodeId(3);
        let class = ClassBindingProjection {
            class_code: code,
            namespace: LocalLocation(2),
            slots: vec![],
        };
        let mut namespace = layout.clone();
        namespace.class_bindings = Some(ClassBindingProjection {
            namespace: LocalLocation(1),
            ..class.clone()
        });
        invalid.push(("class namespace", namespace));
        let mut current = layout.clone();
        current.cellvars.push(ClosureSlot {
            logical_name: "class_cell".into(),
            storage_name: "operand".into(),
            init: ClosureInit::Deferred,
        });
        current.class_bindings = Some(ClassBindingProjection {
            slots: vec![ClassBindingSlotProjection {
                slot: ClassBindingSlotId {
                    class_code: code,
                    index: 0,
                },
                storage: ClassBindingStorage::Cell(CellLocation::Owned(0)),
            }],
            ..class.clone()
        });
        invalid.push(("class current", current));
        let mut cell = layout.clone();
        cell.cellvars.push(ClosureSlot {
            logical_name: "lexical".into(),
            storage_name: "operand".into(),
            init: ClosureInit::Deferred,
        });
        invalid.push(("owned raw cell", cell));
        let mut control = layout.clone();
        control
            .block_parameter_roles
            .push(ResolvedBlockParameterRole {
                location: NameLocation::Local(LocalLocation(1)),
                role: BlockParamRole::Exception,
            });
        invalid.push(("control", control));
        let mut abi = layout.clone();
        abi.generator_resume_abi = Some(GeneratorResumeAbi {
            params: vec![GeneratorResumeParamBinding {
                role: GeneratorResumeParamRole::for_kind(FunctionKind::Generator)[0],
                name: "operand".into(),
            }],
        });
        invalid.push(("resume ABI", abi));
        for (role, invalid) in invalid {
            assert!(invalid.is_expression_temporary(LocalLocation(1)));
            assert!(take.validate_resolved(&invalid).is_err(), "{role}");
        }
        let mut unmarked = layout.clone();
        unmarked.expression_temporaries.clear();
        assert!(take.validate_resolved(&unmarked).is_err());
        for location in [
            NameLocation::Local(LocalLocation(99)),
            NameLocation::Preserved(PreservedLocation(0)),
            NameLocation::Cell(CellLocation::Owned(0)),
            NameLocation::Constant(0),
        ] {
            let redirected = TakeOperand::<OperandTestExpr>::new(
                operand_test_name("operand", 1).with_location(location),
            );
            assert!(redirected.validate_resolved(&layout).is_err());
        }
    }

    #[test]
    fn preserved_operand_take_requires_its_exact_non_source_owner_role() {
        use super::super::*;
        let location = PreservedLocation(0);
        let mut layout = StorageLayout {
            preserved_slots: vec![PreservedSlot {
                generator_control: None,
                logical_name: "operand".into(),
                storage_name: "physical_operand".into(),
                storage: PreservedSlotStorage::PyObjectOrNull,
                init: ClosureInit::Deferred,
            }],
            ..Default::default()
        };
        let params = GeneratorResumeParamRole::for_kind(FunctionKind::Generator)
            .iter()
            .enumerate()
            .map(|(index, role)| GeneratorResumeParamBinding {
                role: *role,
                name: format!("abi_{index}"),
            })
            .collect::<Vec<_>>();
        layout.stack_slots = params.iter().map(|param| param.name.clone()).collect();
        layout.generator_resume_abi = Some(GeneratorResumeAbi { params });
        layout.mark_expression_temporary(location);
        let name = operand_test_name("display_alias_is_not_authority", 0)
            .with_location(NameLocation::Preserved(location));
        let take = TakeOperand::<OperandTestExpr>::new(name.clone());
        assert_eq!(
            take.validate_resolved(&layout).unwrap(),
            OperandLocation::Preserved(location)
        );
        for invalidation in 0..7 {
            let mut invalid = layout.clone();
            match invalidation {
                0 => invalid.expression_temporaries.clear(),
                1 => invalid.preserved_slots[0].storage = PreservedSlotStorage::PyCellObject,
                2 => invalid.preserved_slots[0].storage = PreservedSlotStorage::I64,
                3 => invalid.preserved_slots[0].init = ClosureInit::Parameter,
                4 => {
                    invalid.preserved_slots[0].generator_control =
                        Some(GeneratorControlRole::Delegate)
                }
                5 => invalid.generator_resume_abi = None,
                6 => invalid
                    .block_parameter_roles
                    .push(ResolvedBlockParameterRole {
                        location: name.location,
                        role: BlockParamRole::Exception,
                    }),
                _ => unreachable!(),
            }
            assert!(
                take.validate_resolved(&invalid).is_err(),
                "invalidation {invalidation}"
            );
        }
        let insert = ComprehensionInsert::new(
            ComprehensionInsertKind::ListAppend,
            name,
            None,
            Box::new(OperandTestExpr::Take(take)),
        );
        assert!(insert.validate_resolved(&layout).is_err());
        layout.set_stack_slots(vec![]);
        assert!(layout.is_expression_temporary(location));
    }

    #[test]
    fn operand_take_shared_walk_preserves_nested_evaluation_order() {
        use super::super::{
            BlockTerm, LocalLocation, NameLocation, OperandLocation, PreservedLocation,
        };
        let take =
            |index| OperandTestExpr::Take(TakeOperand::new(operand_test_name("alias", index)));
        let expr = OperandTestExpr::Sequence(vec![
            take(3),
            OperandTestExpr::Sequence(vec![
                OperandTestExpr::Take(TakeOperand::new(
                    operand_test_name("alias", 1)
                        .with_location(NameLocation::Preserved(PreservedLocation(1))),
                )),
                take(2),
            ]),
        ]);
        let mut actual = Vec::new();
        visit_operand_takes(&expr, |location| actual.push(location));
        assert_eq!(
            actual,
            vec![
                OperandLocation::Local(LocalLocation(3)),
                OperandLocation::Preserved(PreservedLocation(1)),
                OperandLocation::Local(LocalLocation(2))
            ]
        );
        actual.clear();
        visit_term_operand_takes(&BlockTerm::Return(expr), |location| actual.push(location));
        assert_eq!(
            actual,
            vec![
                OperandLocation::Local(LocalLocation(3)),
                OperandLocation::Preserved(PreservedLocation(1)),
                OperandLocation::Local(LocalLocation(2))
            ]
        );
    }

    #[test]
    fn comprehension_insert_rejects_nested_take_of_its_borrowed_container() {
        use super::super::LocalLocation;
        let mut layout = operand_test_layout();
        layout.expression_temporaries.push(LocalLocation(0).into());
        let take = |index| {
            OperandTestExpr::Take(TakeOperand::new(operand_test_name(
                "different_alias",
                index,
            )))
        };
        let mut op = ComprehensionInsert::new(
            ComprehensionInsertKind::DictSetItem,
            operand_test_name("container_alias", 1),
            Some(Box::new(take(0))),
            Box::new(OperandTestExpr::Leaf),
        );
        assert_eq!(
            op.validate_resolved(&layout).unwrap(),
            LocalLocation(1).into()
        );
        op.value = Box::new(OperandTestExpr::Sequence(vec![
            OperandTestExpr::Leaf,
            take(1),
        ]));
        assert!(op.validate_resolved(&layout).is_err());
        op.value = Box::new(OperandTestExpr::Leaf);
        op.key = Some(Box::new(OperandTestExpr::Sequence(vec![take(1)])));
        assert!(op.validate_resolved(&layout).is_err());
        op.key = None;
        assert!(op.validate_shape().is_err());
        op.kind = ComprehensionInsertKind::ListAppend;
        assert!(op.validate_resolved(&layout).is_ok());
        op.key = Some(Box::new(OperandTestExpr::Leaf));
        assert!(op.validate_shape().is_err());
        op.kind = ComprehensionInsertKind::SetAdd;
        assert!(op.validate_shape().is_err());
    }

    #[test]
    fn comprehension_insert_maps_container_and_visits_key_before_value() {
        struct Children(Vec<&'static str>);
        impl Visit<ChildInstr> for Children {
            fn visit_instr(&mut self, value: &ChildInstr) {
                self.0.push(value.0);
            }
        }
        struct Rename;
        impl MapInstr<ChildInstr, MappedChildInstr> for Rename {
            fn map_instr(&mut self, value: ChildInstr) -> MappedChildInstr {
                MappedChildInstr(value.0)
            }
            fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
                format!("renamed_{}", name.id_str()).into()
            }
        }
        impl TryMapInstr<ChildInstr, MappedChildInstr, ()> for Rename {
            fn try_map_instr(&mut self, value: ChildInstr) -> Result<MappedChildInstr, ()> {
                Ok(MappedChildInstr(value.0))
            }
            fn try_map_name(&mut self, name: UnresolvedName) -> Result<UnresolvedName, ()> {
                Ok(format!("renamed_{}", name.id_str()).into())
            }
        }
        let op = ComprehensionInsert::new(
            ComprehensionInsertKind::DictSetItem,
            "container".into(),
            Some(Box::new(ChildInstr("key"))),
            Box::new(ChildInstr("value")),
        )
        .with_extra("producer");
        let mut children = Children(Vec::new());
        op.visit_children(&mut children);
        assert_eq!(children.0, vec!["key", "value"]);
        for same in [
            op.clone().map_same_children(&mut |value| value),
            op.clone()
                .try_map_same_children(&mut |value| Ok::<_, ()>(value))
                .unwrap(),
        ] {
            assert_eq!(*same.extra(), "producer");
            assert_eq!(same.container.id_str(), "container");
        }
        for mapped in [
            op.clone().map_children(&mut Rename),
            op.try_map_children(&mut Rename).unwrap(),
        ] {
            assert_eq!(mapped.kind, ComprehensionInsertKind::DictSetItem);
            assert_eq!(mapped.container.id_str(), "renamed_container");
            assert_eq!(mapped.key.as_ref().unwrap().0, "key");
            assert_eq!(mapped.value.0, "value");
            assert_eq!(*mapped.extra(), "");
        }
        let take = TakeOperand::<ChildInstr>::new("operand").with_extra("take");
        let same = take.clone().map_same_children(&mut |value| value);
        assert_eq!(*same.extra(), "take");
        let mapped = take.map_children(&mut Rename);
        assert_eq!(mapped.name.id_str(), "renamed_operand");
        assert_eq!(*mapped.extra(), "");
    }

    #[test]
    fn ruff_dictionary_payload_maps_children_without_ast_roundtrips() {
        struct Children(Vec<&'static str>);
        impl Visit<ChildInstr> for Children {
            fn visit_instr(&mut self, value: &ChildInstr) {
                self.0.push(value.0);
            }
        }
        struct MapChild;
        impl MapInstr<ChildInstr, MappedChildInstr> for MapChild {
            fn map_instr(&mut self, value: ChildInstr) -> MappedChildInstr {
                MappedChildInstr(value.0)
            }
            fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
                name
            }
        }
        impl TryMapInstr<ChildInstr, MappedChildInstr, ()> for MapChild {
            fn try_map_instr(&mut self, value: ChildInstr) -> Result<MappedChildInstr, ()> {
                Ok(MappedChildInstr(value.0))
            }
            fn try_map_name(&mut self, name: UnresolvedName) -> Result<UnresolvedName, ()> {
                Ok(name)
            }
        }
        let dict = ExprDict::new(vec![
            ExprDictItem {
                key: Some(ChildInstr("key")),
                value: ChildInstr("value"),
            },
            ExprDictItem {
                key: None,
                value: ChildInstr("mapping"),
            },
        ])
        .with_extra("source-dict");
        let mut children = Children(Vec::new());
        dict.visit_children(&mut children);
        assert_eq!(children.0, vec!["key", "value", "mapping"]);
        for same in [
            dict.clone().map_same_children(&mut |value| value),
            dict.clone()
                .try_map_same_children(&mut |value| Ok::<_, ()>(value))
                .unwrap(),
        ] {
            assert_eq!(*same.extra(), "source-dict");
            assert!(same.items[1].key.is_none());
        }
        for mapped in [
            dict.clone().map_children(&mut MapChild),
            dict.try_map_children(&mut MapChild).unwrap(),
        ] {
            assert_eq!(mapped.items[0].key.as_ref().unwrap().0, "key");
            assert_eq!(mapped.items[0].value.0, "value");
            assert_eq!(mapped.items[1].value.0, "mapping");
            assert!(mapped.items[1].key.is_none());
            assert_eq!(*mapped.extra(), "");
        }
    }
}
