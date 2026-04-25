use cranelift_codegen::ir;
use soac_ir_typed::PyObjFacts;
pub use soac_ir_typed::TypedResultDemand as ResultDemand;

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
    pub const I64: Self = Self {
        min: i64::MIN as i128,
        max: i64::MAX as i128,
    };

    pub const fn exact(value: i128) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    pub const fn is_within(self, outer: Self) -> bool {
        self.min >= outer.min && self.max <= outer.max
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        Some(Self {
            min: self.min.checked_add(rhs.min)?,
            max: self.max.checked_add(rhs.max)?,
        })
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        Some(Self {
            min: self.min.checked_sub(rhs.max)?,
            max: self.max.checked_sub(rhs.min)?,
        })
    }

    pub fn checked_mul(self, rhs: Self) -> Option<Self> {
        let products = [
            self.min.checked_mul(rhs.min)?,
            self.min.checked_mul(rhs.max)?,
            self.max.checked_mul(rhs.min)?,
            self.max.checked_mul(rhs.max)?,
        ];
        Some(Self {
            min: *products.iter().min()?,
            max: *products.iter().max()?,
        })
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

    pub const fn i64_range(range: IntRange) -> Self {
        Self {
            width: IntWidth::I64,
            known_value: None,
            range: Some(range),
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
    PyObject {
        value: ir::Value,
        ownership: ValueOwnership,
        facts: PyObjFacts,
    },
    I32 {
        value: ir::Value,
        facts: IntFacts,
    },
    I64 {
        value: ir::Value,
        facts: IntFacts,
    },
}

impl SoacValue {
    pub fn pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::owned_pyobject(value, facts)
    }

    pub fn pyobject_with_ownership(
        value: ir::Value,
        ownership: ValueOwnership,
        facts: PyObjFacts,
    ) -> Self {
        Self::PyObject {
            value,
            ownership,
            facts,
        }
    }

    pub fn owned_pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::pyobject_with_ownership(value, ValueOwnership::Owned, facts)
    }

    pub fn borrowed_pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::pyobject_with_ownership(value, ValueOwnership::Borrowed, facts)
    }

    pub fn immortal_pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::pyobject_with_ownership(value, ValueOwnership::Immortal, facts)
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

    pub const fn as_pyobject(self) -> Option<(ir::Value, ValueOwnership, PyObjFacts)> {
        match self {
            Self::PyObject {
                value,
                ownership,
                facts,
            } => Some((value, ownership, facts)),
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
    pub fn expect_pyobject(self, context: &str) -> (ir::Value, ValueOwnership, PyObjFacts) {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueOwnership {
    Owned,
    Borrowed,
    Immortal,
}

impl ValueOwnership {
    pub const fn is_owned(self) -> bool {
        matches!(self, Self::Owned)
    }

    pub const fn can_satisfy_pyobject_demand(self, demand: ResultDemand) -> bool {
        match demand {
            ResultDemand::EffectOnly
            | ResultDemand::I32Bool01
            | ResultDemand::I64
            | ResultDemand::I64Index => false,
            ResultDemand::PyObject { borrowed_ok } => {
                borrowed_ok || matches!(self, Self::Owned | Self::Immortal)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmitResult {
    NoValue,
    PyObject {
        value: ir::Value,
        ownership: ValueOwnership,
        facts: PyObjFacts,
    },
    I32 {
        value: ir::Value,
        facts: IntFacts,
    },
    I64 {
        value: ir::Value,
        facts: IntFacts,
    },
}

impl EmitResult {
    pub const fn no_value() -> Self {
        Self::NoValue
    }

    pub fn pyobject(value: ir::Value, ownership: ValueOwnership, facts: PyObjFacts) -> Self {
        Self::PyObject {
            value,
            ownership,
            facts,
        }
    }

    pub fn owned_pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::pyobject(value, ValueOwnership::Owned, facts)
    }

    pub fn borrowed_pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::pyobject(value, ValueOwnership::Borrowed, facts)
    }

    pub fn immortal_pyobject(value: ir::Value, facts: PyObjFacts) -> Self {
        Self::pyobject(value, ValueOwnership::Immortal, facts)
    }

    pub fn i32(value: ir::Value, facts: IntFacts) -> Self {
        assert_eq!(facts.width, IntWidth::I32, "I32 EmitResult needs I32 facts");
        Self::I32 { value, facts }
    }

    pub fn i64(value: ir::Value, facts: IntFacts) -> Self {
        assert_eq!(facts.width, IntWidth::I64, "I64 EmitResult needs I64 facts");
        Self::I64 { value, facts }
    }

    pub const fn has_value(self) -> bool {
        !matches!(self, Self::NoValue)
    }

    pub const fn as_pyobject(self) -> Option<(ir::Value, ValueOwnership, PyObjFacts)> {
        match self {
            Self::PyObject {
                value,
                ownership,
                facts,
            } => Some((value, ownership, facts)),
            Self::NoValue | Self::I32 { .. } | Self::I64 { .. } => None,
        }
    }

    pub const fn as_i32(self) -> Option<(ir::Value, IntFacts)> {
        match self {
            Self::I32 { value, facts } => Some((value, facts)),
            Self::NoValue | Self::PyObject { .. } | Self::I64 { .. } => None,
        }
    }

    pub const fn as_i64(self) -> Option<(ir::Value, IntFacts)> {
        match self {
            Self::I64 { value, facts } => Some((value, facts)),
            Self::NoValue | Self::PyObject { .. } | Self::I32 { .. } => None,
        }
    }

    #[track_caller]
    pub fn expect_pyobject(self, context: &str) -> (ir::Value, ValueOwnership, PyObjFacts) {
        self.as_pyobject()
            .unwrap_or_else(|| panic!("{context}: expected PyObject result, got {self:?}"))
    }

    #[track_caller]
    pub fn expect_i32_bool01(self, context: &str) -> ir::Value {
        let (value, facts) = self
            .as_i32()
            .unwrap_or_else(|| panic!("{context}: expected I32 result, got {self:?}"));
        assert!(
            facts.is_i32_bool01(),
            "{context}: expected normalized I32 0/1 result, got {facts:?}"
        );
        value
    }

    #[track_caller]
    pub fn expect_i64(self, context: &str) -> (ir::Value, IntFacts) {
        self.as_i64()
            .unwrap_or_else(|| panic!("{context}: expected I64 result, got {self:?}"))
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
        let borrowed_py = SoacValue::borrowed_pyobject(value(4), PyObjFacts::unknown());
        let immortal_py = SoacValue::immortal_pyobject(value(5), PyObjFacts::bool_object());
        let i32_value = SoacValue::i32(value(2), IntFacts::i32_bool01());
        let i64_value = SoacValue::i64(value(3), IntFacts::i64_known(42));

        assert_eq!(py.repr(), SoacRepr::PyObject);
        assert_eq!(i32_value.repr(), SoacRepr::I32);
        assert_eq!(i64_value.repr(), SoacRepr::I64);
        assert_eq!(py.raw_value(), value(1));
        assert_eq!(
            py.as_pyobject(),
            Some((
                value(1),
                ValueOwnership::Owned,
                PyObjFacts::none_singleton()
            ))
        );
        assert_eq!(
            borrowed_py.as_pyobject(),
            Some((value(4), ValueOwnership::Borrowed, PyObjFacts::unknown()))
        );
        assert_eq!(
            immortal_py.as_pyobject(),
            Some((
                value(5),
                ValueOwnership::Immortal,
                PyObjFacts::bool_object()
            ))
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

    #[test]
    fn result_demand_records_whether_consumers_need_a_value() {
        assert!(!ResultDemand::EffectOnly.needs_value());
        assert!(!ResultDemand::EffectOnly.borrowed_ok());
        assert!(ResultDemand::I32_BOOL01.needs_value());
        assert!(!ResultDemand::I32_BOOL01.borrowed_ok());
        assert!(ResultDemand::I64_VALUE.needs_value());
        assert!(!ResultDemand::I64_VALUE.borrowed_ok());
        assert!(ResultDemand::I64_INDEX.needs_value());
        assert!(!ResultDemand::I64_INDEX.borrowed_ok());
        assert!(ResultDemand::PYOBJECT_OWNED.needs_value());
        assert!(!ResultDemand::PYOBJECT_OWNED.borrowed_ok());
        assert!(ResultDemand::PYOBJECT_BORROWED_OK.needs_value());
        assert!(ResultDemand::PYOBJECT_BORROWED_OK.borrowed_ok());
    }

    #[test]
    fn value_ownership_checks_pyobject_demand_compatibility() {
        assert!(ValueOwnership::Owned.is_owned());
        assert!(!ValueOwnership::Borrowed.is_owned());
        assert!(ValueOwnership::Owned.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED));
        assert!(ValueOwnership::Immortal.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED));
        assert!(
            !ValueOwnership::Borrowed.can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_OWNED)
        );
        assert!(
            ValueOwnership::Borrowed
                .can_satisfy_pyobject_demand(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert!(!ValueOwnership::Owned.can_satisfy_pyobject_demand(ResultDemand::EffectOnly));
        assert!(!ValueOwnership::Owned.can_satisfy_pyobject_demand(ResultDemand::I32_BOOL01));
        assert!(!ValueOwnership::Owned.can_satisfy_pyobject_demand(ResultDemand::I64_VALUE));
        assert!(!ValueOwnership::Owned.can_satisfy_pyobject_demand(ResultDemand::I64_INDEX));
    }

    #[test]
    fn emit_result_preserves_value_representation_and_ownership() {
        let py = EmitResult::owned_pyobject(value(8), PyObjFacts::bool_object());
        let borrowed_py = EmitResult::borrowed_pyobject(value(9), PyObjFacts::unknown());
        let immortal_py = EmitResult::immortal_pyobject(value(10), PyObjFacts::none_singleton());
        let i32_value = EmitResult::i32(value(11), IntFacts::i32_bool01());
        let i64_value = EmitResult::i64(value(12), IntFacts::i64_known(99));

        assert!(py.has_value());
        assert!(!EmitResult::no_value().has_value());
        assert_eq!(
            py.as_pyobject(),
            Some((value(8), ValueOwnership::Owned, PyObjFacts::bool_object()))
        );
        assert_eq!(
            borrowed_py.expect_pyobject("borrowed result"),
            (value(9), ValueOwnership::Borrowed, PyObjFacts::unknown())
        );
        assert_eq!(
            immortal_py.as_pyobject(),
            Some((
                value(10),
                ValueOwnership::Immortal,
                PyObjFacts::none_singleton()
            ))
        );
        assert_eq!(
            i32_value.as_i32(),
            Some((value(11), IntFacts::i32_bool01()))
        );
        assert_eq!(
            i64_value.as_i64(),
            Some((value(12), IntFacts::i64_known(99)))
        );
        assert_eq!(EmitResult::no_value().as_pyobject(), None);
    }

    #[test]
    fn emit_result_expect_i32_bool01_returns_normalized_truth_value() {
        let truth = EmitResult::i32(value(13), IntFacts::i32_bool01());

        assert_eq!(truth.expect_i32_bool01("branch demand"), value(13));
    }

    #[test]
    #[should_panic(expected = "I32 EmitResult needs I32 facts")]
    fn emit_result_i32_constructor_rejects_i64_facts() {
        EmitResult::i32(value(14), IntFacts::i64_unknown());
    }

    #[test]
    #[should_panic(expected = "expected PyObject result")]
    fn emit_result_expect_pyobject_rejects_no_value() {
        EmitResult::no_value().expect_pyobject("return boundary");
    }

    #[test]
    #[should_panic(expected = "expected normalized I32 0/1 result")]
    fn emit_result_expect_i32_bool01_rejects_unknown_i32() {
        EmitResult::i32(value(15), IntFacts::i32_unknown()).expect_i32_bool01("branch demand");
    }
}
