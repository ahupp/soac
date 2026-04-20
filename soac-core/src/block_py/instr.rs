use super::instr_macro::{define_instr, define_ruff_instr};
use super::{
    CallArgKeyword, CallArgPositional, CellLocation, ChildVisitable, FunctionKind, HasMeta, Instr,
    MapInstr, Mappable, Meta, NameLike, RuntimeFunctionId, TryMapInstr, WithMeta,
};
use ruff_python_ast::{self as ast};
use std::fmt;

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

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Call<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub func: Box<E>,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
}

impl<E: Instr + fmt::Debug> fmt::Debug for Call<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}(", self.func)?;
        let mut first = true;
        for arg in &self.args {
            if !first {
                write!(f, ", ")?;
            }
            first = false;
            match arg {
                CallArgPositional::Positional(expr) => write!(f, "{expr:?}")?,
                CallArgPositional::Starred(expr) => write!(f, "*{expr:?}")?,
            }
        }
        for keyword in &self.keywords {
            if !first {
                write!(f, ", ")?;
            }
            first = false;
            match keyword {
                CallArgKeyword::Named { arg, value } => write!(f, "{arg}={value:?}")?,
                CallArgKeyword::Starred(value) => write!(f, "**{value:?}")?,
            }
        }
        write!(f, ")")
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
        })
    }
}

define_instr! {
    pub struct CalleeFunctionId<E> {
        value: Box<E>,
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CallDirect<E: Instr> {
    _meta: Meta,
    pub extra: E::Extra,
    pub callable: Box<E>,
    pub function_id: RuntimeFunctionId,
    pub args: Vec<CallArgPositional<E>>,
    pub keywords: Vec<CallArgKeyword<E>>,
}

impl<E: Instr + fmt::Debug> fmt::Debug for CallDirect<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CallDirect({}, {:?}", self.function_id, self.callable)?;
        for arg in &self.args {
            write!(f, ", ")?;
            match arg {
                CallArgPositional::Positional(expr) => write!(f, "{expr:?}")?,
                CallArgPositional::Starred(expr) => write!(f, "*{expr:?}")?,
            }
        }
        for keyword in &self.keywords {
            write!(f, ", ")?;
            match keyword {
                CallArgKeyword::Named { arg, value } => write!(f, "{arg}={value:?}")?,
                CallArgKeyword::Starred(value) => write!(f, "**{value:?}")?,
            }
        }
        write!(f, ")")
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

define_instr! {
    pub struct DelItem<E> {
        value: Box<E>,
        index: Box<E>,
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Load<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
}

impl<I: Instr> fmt::Debug for Load<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name.pretty_id())
    }
}

impl<I: Instr> Load<I> {
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
        })
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Store<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
    pub value: Box<I>,
}

impl<I: Instr> fmt::Debug for Store<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.name.pretty_id() == self.name.id_str() {
            write!(f, "StoreName({:?}, {:?})", self.name.id_str(), self.value)
        } else {
            write!(
                f,
                "StoreLocation({}, {:?})",
                self.name.pretty_id(),
                self.value
            )
        }
    }
}

impl<I: Instr> Store<I> {
    pub fn new(name: impl Into<I::Name>, value: impl Into<Box<I>>) -> Self {
        Self {
            _meta: Meta::default(),
            extra: Default::default(),
            name: name.into(),
            value: value.into(),
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
        })
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Del<I: Instr> {
    _meta: Meta,
    pub extra: I::Extra,
    pub name: I::Name,
    pub quietly: bool,
}

impl<I: Instr> fmt::Debug for Del<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Del")
            .field("name", &self.name.pretty_id())
            .field("quietly", &self.quietly)
            .finish()
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
    }
}

impl<E: Instr> MakeFunctionWithClosure<E> {
    pub fn function_id(&self) -> RuntimeFunctionId {
        self.function_id
    }

    pub fn set_function_id(&mut self, function_id: RuntimeFunctionId) {
        self.function_id = function_id;
    }
}

define_instr! {
    pub struct Await<E> {
        value: Box<E>,
    }
}

define_instr! {
    pub struct Yield<E> {
        value: Box<E>,
    }
}

define_instr! {
    pub struct YieldFrom<E> {
        value: Box<E>,
    }
}

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

define_ruff_instr! {
    pub struct ExprDict {
        items: Vec<ast::DictItem>,
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
}
