use cranelift_codegen::ir;
use soac_blockpy::passes::PyObjFacts;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntWidth {
    I32,
    I64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntRange {
    pub min: i128,
    pub max: i128,
}

impl IntRange {
    pub const ZERO_OR_ONE: Self = Self { min: 0, max: 1 };

    pub const fn exact(value: i128) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    pub const fn is_within(self, outer: Self) -> bool {
        self.min >= outer.min && self.max <= outer.max
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntFacts {
    pub width: IntWidth,
    pub known_value: Option<i128>,
    pub range: Option<IntRange>,
}

impl IntFacts {
    pub const fn i32_unknown() -> Self {
        Self {
            width: IntWidth::I32,
            known_value: None,
            range: None,
        }
    }

    pub const fn i32_known(value: i32) -> Self {
        Self {
            width: IntWidth::I32,
            known_value: Some(value as i128),
            range: Some(IntRange::exact(value as i128)),
        }
    }

    pub const fn i32_bool01() -> Self {
        Self {
            width: IntWidth::I32,
            known_value: None,
            range: Some(IntRange::ZERO_OR_ONE),
        }
    }

    pub const fn i64_unknown() -> Self {
        Self {
            width: IntWidth::I64,
            known_value: None,
            range: None,
        }
    }

    pub const fn i64_known(value: i64) -> Self {
        Self {
            width: IntWidth::I64,
            known_value: Some(value as i128),
            range: Some(IntRange::exact(value as i128)),
        }
    }

    pub const fn is_i32_bool01(self) -> bool {
        if !matches!(self.width, IntWidth::I32) {
            return false;
        }
        if let Some(value) = self.known_value {
            if value != 0 && value != 1 {
                return false;
            }
        }
        match self.range {
            Some(range) => range.is_within(IntRange::ZERO_OR_ONE),
            None => matches!(self.known_value, Some(0 | 1)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoacRepr {
    PyObject,
    I32,
    I64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoacValue {
    PyObject { value: ir::Value, facts: PyObjFacts },
    I32 { value: ir::Value, facts: IntFacts },
    I64 { value: ir::Value, facts: IntFacts },
}

impl SoacValue {
    pub fn pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::PyObject { value, facts }
    }

    pub fn i32(value: ir::Value, facts: IntFacts) -> Self {
        assert_eq!(facts.width, IntWidth::I32, "I32 SoacValue needs I32 facts");
        Self::I32 { value, facts }
    }

    pub fn i64(value: ir::Value, facts: IntFacts) -> Self {
        assert_eq!(facts.width, IntWidth::I64, "I64 SoacValue needs I64 facts");
        Self::I64 { value, facts }
    }

    pub const fn repr(self) -> SoacRepr {
        match self {
            Self::PyObject { .. } => SoacRepr::PyObject,
            Self::I32 { .. } => SoacRepr::I32,
            Self::I64 { .. } => SoacRepr::I64,
        }
    }

    pub const fn raw_value(self) -> ir::Value {
        match self {
            Self::PyObject { value, .. } | Self::I32 { value, .. } | Self::I64 { value, .. } => {
                value
            }
        }
    }

    pub const fn as_pyobject(self) -> Option<(ir::Value, PyObjFacts)> {
        match self {
            Self::PyObject { value, facts } => Some((value, facts)),
            Self::I32 { .. } | Self::I64 { .. } => None,
        }
    }

    pub const fn as_i32(self) -> Option<(ir::Value, IntFacts)> {
        match self {
            Self::I32 { value, facts } => Some((value, facts)),
            Self::PyObject { .. } | Self::I64 { .. } => None,
        }
    }

    pub const fn as_i64(self) -> Option<(ir::Value, IntFacts)> {
        match self {
            Self::I64 { value, facts } => Some((value, facts)),
            Self::PyObject { .. } | Self::I32 { .. } => None,
        }
    }

    #[track_caller]
    pub fn expect_pyobject(self, context: &str) -> (ir::Value, PyObjFacts) {
        self.as_pyobject()
            .unwrap_or_else(|| panic!("{context}: expected PyObject value, got {:?}", self.repr()))
    }

    #[track_caller]
    pub fn expect_i32(self, context: &str) -> (ir::Value, IntFacts) {
        self.as_i32()
            .unwrap_or_else(|| panic!("{context}: expected I32 value, got {:?}", self.repr()))
    }

    #[track_caller]
    pub fn expect_i64(self, context: &str) -> (ir::Value, IntFacts) {
        self.as_i64()
            .unwrap_or_else(|| panic!("{context}: expected I64 value, got {:?}", self.repr()))
    }

    #[track_caller]
    pub fn expect_i32_bool01(self, context: &str) -> ir::Value {
        let (value, facts) = self.expect_i32(context);
        assert!(
            facts.is_i32_bool01(),
            "{context}: expected normalized I32 0/1 value, got {facts:?}"
        );
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(index: u32) -> ir::Value {
        ir::Value::from_u32(index)
    }

    #[test]
    fn int_facts_record_width_known_values_and_ranges() {
        assert_eq!(
            IntFacts::i32_unknown(),
            IntFacts {
                width: IntWidth::I32,
                known_value: None,
                range: None,
            }
        );
        assert_eq!(
            IntFacts::i32_known(7),
            IntFacts {
                width: IntWidth::I32,
                known_value: Some(7),
                range: Some(IntRange::exact(7)),
            }
        );
        assert_eq!(
            IntFacts::i64_known(-3),
            IntFacts {
                width: IntWidth::I64,
                known_value: Some(-3),
                range: Some(IntRange::exact(-3)),
            }
        );
    }

    #[test]
    fn bool01_is_i32_range_invariant() {
        assert!(IntFacts::i32_bool01().is_i32_bool01());
        assert!(IntFacts::i32_known(0).is_i32_bool01());
        assert!(IntFacts::i32_known(1).is_i32_bool01());
        assert!(!IntFacts::i32_known(2).is_i32_bool01());
        assert!(!IntFacts::i64_known(1).is_i32_bool01());
        assert!(!IntFacts::i32_unknown().is_i32_bool01());
    }

    #[test]
    fn soac_value_preserves_representation_specific_facts() {
        let py = SoacValue::pyobject(value(1), PyObjFacts::none_singleton());
        let i32_value = SoacValue::i32(value(2), IntFacts::i32_bool01());
        let i64_value = SoacValue::i64(value(3), IntFacts::i64_known(42));

        assert_eq!(py.repr(), SoacRepr::PyObject);
        assert_eq!(i32_value.repr(), SoacRepr::I32);
        assert_eq!(i64_value.repr(), SoacRepr::I64);
        assert_eq!(py.raw_value(), value(1));
        assert_eq!(
            py.as_pyobject(),
            Some((value(1), PyObjFacts::none_singleton()))
        );
        assert_eq!(i32_value.as_i32(), Some((value(2), IntFacts::i32_bool01())));
        assert_eq!(
            i64_value.as_i64(),
            Some((value(3), IntFacts::i64_known(42)))
        );
        assert_eq!(py.as_i32(), None);
        assert_eq!(i32_value.as_pyobject(), None);
    }

    #[test]
    fn expect_i32_bool01_returns_normalized_truth_value() {
        let truth = SoacValue::i32(value(4), IntFacts::i32_bool01());

        assert_eq!(truth.expect_i32_bool01("branch condition"), value(4));
    }

    #[test]
    #[should_panic(expected = "I32 SoacValue needs I32 facts")]
    fn i32_constructor_rejects_i64_facts() {
        SoacValue::i32(value(5), IntFacts::i64_unknown());
    }

    #[test]
    #[should_panic(expected = "expected normalized I32 0/1 value")]
    fn expect_i32_bool01_rejects_unknown_i32() {
        SoacValue::i32(value(6), IntFacts::i32_unknown()).expect_i32_bool01("branch condition");
    }

    #[test]
    #[should_panic(expected = "expected PyObject value")]
    fn expect_pyobject_rejects_i32() {
        SoacValue::i32(value(7), IntFacts::i32_bool01()).expect_pyobject("python boundary");
    }
}
