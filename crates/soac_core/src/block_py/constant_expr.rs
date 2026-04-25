use super::{LiteralValue, RuntimeName};
use std::fmt;

#[derive(Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ConstantExpr {
    Literal(LiteralValue),
    RuntimeName(RuntimeName),
}

impl fmt::Debug for ConstantExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(value) => value.fmt(f),
            Self::RuntimeName(name) => write!(f, "RuntimeName({})", name.name()),
        }
    }
}

impl From<LiteralValue> for ConstantExpr {
    fn from(value: LiteralValue) -> Self {
        Self::Literal(value)
    }
}

impl From<RuntimeName> for ConstantExpr {
    fn from(value: RuntimeName) -> Self {
        Self::RuntimeName(value)
    }
}
