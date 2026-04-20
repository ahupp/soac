use super::*;
use std::fmt;

#[derive(Clone, derive_more::From, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct LiteralValue {
    _meta: Meta,
    pub literal: Literal,
}

impl fmt::Debug for LiteralValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_tuple("LiteralValue");
        debug.field(&self.literal);
        debug.finish()
    }
}

impl PrettyPrint for LiteralValue {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        std::fmt::Write::write_fmt(printer, format_args!("{:?}", self.literal))
    }
}

impl LiteralValue {
    pub fn new(literal: impl Into<Literal>) -> Self {
        Self {
            _meta: Meta::default(),
            literal: literal.into(),
        }
    }
}

impl HasMeta for LiteralValue {
    fn meta(&self) -> Meta {
        self._meta.clone()
    }
}

impl WithMeta for LiteralValue {
    fn with_meta(mut self, meta: Meta) -> Self {
        self._meta = meta;
        self
    }
}

impl<E: Instr> ChildVisitable<E> for LiteralValue {
    fn visit_children<V>(&self, visitor: &mut V)
    where
        V: Visit<E> + ?Sized,
    {
        let _ = visitor;
    }

    fn visit_children_mut<V>(&mut self, visitor: &mut V)
    where
        V: VisitMut<E> + ?Sized,
    {
        let _ = visitor;
    }
}

impl<E: Instr> Mappable<E> for LiteralValue {
    type Mapped<T: Instr> = LiteralValue;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>,
    {
        let _ = map;
        self
    }

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>,
    {
        let _ = map;
        Ok(self)
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
    E: Instr + From<LiteralValue>,
{
    E::from(literal_value(literal, meta))
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StringLiteral {
    pub value: String,
}

impl fmt::Debug for StringLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BytesLiteral {
    pub value: Vec<u8>,
}

impl fmt::Debug for BytesLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct NumberLiteral {
    pub value: NumberLiteralValue,
}

impl fmt::Debug for NumberLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum NumberLiteralValue {
    Int(IntLiteral),
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

#[derive(Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IntLiteral {
    decimal: String,
}

impl IntLiteral {
    pub fn from_decimal(decimal: impl Into<String>) -> Self {
        Self {
            decimal: decimal.into(),
        }
    }

    pub fn from_i64(value: i64) -> Self {
        Self::from_decimal(value.to_string())
    }

    pub fn as_decimal(&self) -> &str {
        &self.decimal
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.decimal.parse().ok()
    }
}

impl From<ruff_python_ast::Int> for IntLiteral {
    fn from(value: ruff_python_ast::Int) -> Self {
        Self::from_decimal(value.to_string())
    }
}

impl fmt::Debug for IntLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_decimal())
    }
}

impl fmt::Display for IntLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_decimal())
    }
}
