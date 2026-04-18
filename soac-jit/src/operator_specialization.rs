use soac_core::block_py::{BinOpKind, UnaryOpKind};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExactTypeTag {
    Int = 1,
}

impl ExactTypeTag {
    pub const fn packed(self) -> u64 {
        self as u8 as u64
    }

    pub const fn from_packed(value: u64) -> Option<Self> {
        match value as u8 {
            1 => Some(Self::Int),
            _ => None,
        }
    }
}

pub const UNARY_TAG_SHIFT: u32 = 0;
pub const BINARY_LHS_TAG_SHIFT: u32 = 0;
pub const BINARY_RHS_TAG_SHIFT: u32 = 8;

pub const fn pack_unary_shape(tag: ExactTypeTag) -> u64 {
    tag.packed() << UNARY_TAG_SHIFT
}

pub const fn pack_binary_shape(lhs: ExactTypeTag, rhs: ExactTypeTag) -> u64 {
    (lhs.packed() << BINARY_LHS_TAG_SHIFT) | (rhs.packed() << BINARY_RHS_TAG_SHIFT)
}

pub fn unpack_unary_shape(value: u64) -> Option<ExactTypeTag> {
    ExactTypeTag::from_packed(value >> UNARY_TAG_SHIFT)
}

pub fn unpack_binary_shape(value: u64) -> Option<(ExactTypeTag, ExactTypeTag)> {
    Some((
        ExactTypeTag::from_packed(value >> BINARY_LHS_TAG_SHIFT)?,
        ExactTypeTag::from_packed(value >> BINARY_RHS_TAG_SHIFT)?,
    ))
}

#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExactIntBinaryOpKind {
    Add = 1,
    Sub = 2,
    Mul = 3,
    TrueDiv = 4,
    FloorDiv = 5,
    Mod = 6,
    Pow = 7,
    LShift = 8,
    RShift = 9,
    Or = 10,
    Xor = 11,
    And = 12,
    Eq = 13,
    Ne = 14,
    Lt = 15,
    Le = 16,
    Gt = 17,
    Ge = 18,
    InplaceAdd = 19,
    InplaceSub = 20,
    InplaceMul = 21,
    InplaceTrueDiv = 22,
    InplaceFloorDiv = 23,
    InplaceMod = 24,
    InplacePow = 25,
    InplaceLShift = 26,
    InplaceRShift = 27,
    InplaceOr = 28,
    InplaceXor = 29,
    InplaceAnd = 30,
}

impl ExactIntBinaryOpKind {
    pub fn from_binop_kind(kind: BinOpKind) -> Option<Self> {
        Some(match kind {
            BinOpKind::Add => Self::Add,
            BinOpKind::Sub => Self::Sub,
            BinOpKind::Mul => Self::Mul,
            BinOpKind::TrueDiv => Self::TrueDiv,
            BinOpKind::FloorDiv => Self::FloorDiv,
            BinOpKind::Mod => Self::Mod,
            BinOpKind::Pow => Self::Pow,
            BinOpKind::LShift => Self::LShift,
            BinOpKind::RShift => Self::RShift,
            BinOpKind::Or => Self::Or,
            BinOpKind::Xor => Self::Xor,
            BinOpKind::And => Self::And,
            BinOpKind::Eq => Self::Eq,
            BinOpKind::Ne => Self::Ne,
            BinOpKind::Lt => Self::Lt,
            BinOpKind::Le => Self::Le,
            BinOpKind::Gt => Self::Gt,
            BinOpKind::Ge => Self::Ge,
            BinOpKind::InplaceAdd => Self::InplaceAdd,
            BinOpKind::InplaceSub => Self::InplaceSub,
            BinOpKind::InplaceMul => Self::InplaceMul,
            BinOpKind::InplaceTrueDiv => Self::InplaceTrueDiv,
            BinOpKind::InplaceFloorDiv => Self::InplaceFloorDiv,
            BinOpKind::InplaceMod => Self::InplaceMod,
            BinOpKind::InplacePow => Self::InplacePow,
            BinOpKind::InplaceLShift => Self::InplaceLShift,
            BinOpKind::InplaceRShift => Self::InplaceRShift,
            BinOpKind::InplaceOr => Self::InplaceOr,
            BinOpKind::InplaceXor => Self::InplaceXor,
            BinOpKind::InplaceAnd => Self::InplaceAnd,
            BinOpKind::MatMul | BinOpKind::InplaceMatMul | BinOpKind::Contains | BinOpKind::Is => {
                return None;
            }
        })
    }
}

#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExactIntUnaryOpKind {
    Pos = 1,
    Neg = 2,
    Invert = 3,
    Not = 4,
    Truth = 5,
}

impl ExactIntUnaryOpKind {
    pub fn from_unary_op_kind(kind: UnaryOpKind) -> Self {
        match kind {
            UnaryOpKind::Pos => Self::Pos,
            UnaryOpKind::Neg => Self::Neg,
            UnaryOpKind::Invert => Self::Invert,
            UnaryOpKind::Not => Self::Not,
            UnaryOpKind::Truth => Self::Truth,
        }
    }
}
