#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExactTypeTag {
    Int = 1,
    Str = 2,
    Float = 3,
}

impl ExactTypeTag {
    pub const fn packed(self) -> u64 {
        self as u8 as u64
    }

    pub const fn from_packed(value: u64) -> Option<Self> {
        match value as u8 {
            1 => Some(Self::Int),
            2 => Some(Self::Str),
            3 => Some(Self::Float),
            _ => None,
        }
    }
}

pub const BINARY_LHS_TAG_SHIFT: u32 = 0;
pub const BINARY_RHS_TAG_SHIFT: u32 = 8;

pub const fn pack_binary_shape(lhs: ExactTypeTag, rhs: ExactTypeTag) -> u64 {
    (lhs.packed() << BINARY_LHS_TAG_SHIFT) | (rhs.packed() << BINARY_RHS_TAG_SHIFT)
}

pub fn unpack_binary_shape(value: u64) -> Option<(ExactTypeTag, ExactTypeTag)> {
    Some((
        ExactTypeTag::from_packed(value >> BINARY_LHS_TAG_SHIFT)?,
        ExactTypeTag::from_packed(value >> BINARY_RHS_TAG_SHIFT)?,
    ))
}
