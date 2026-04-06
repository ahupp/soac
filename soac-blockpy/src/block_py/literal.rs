use super::operation_macro::define_operation;
use super::*;
use ruff_python_ast::{self as ast};
use std::fmt;

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
    E: Instr + From<LiteralValue>,
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
