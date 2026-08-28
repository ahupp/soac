use serde::{Deserialize, Serialize};

use crate::{AnnotationOrigin, ContractError, Fingerprint, SourceRange, StaticType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckedFieldPolicy {
    Disabled,
    SupportedAnnotations,
}

impl CheckedFieldPolicy {
    /// The explicit supported subset for field writes. Class participation or
    /// a function signature alone does not enable a field predicate.
    pub fn required_type(
        self,
        origin: AnnotationOrigin,
        value_type: &StaticType,
    ) -> Option<&StaticType> {
        (self == Self::SupportedAnnotations
            && origin == AnnotationOrigin::Explicit
            && value_type.has_supported_value_shape())
        .then_some(value_type)
    }
}

/// A source comment attached to one exact class declaration, not its name.
/// A local opt-out cannot revoke a contract inherited from another class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassPolicyOverride {
    pub class_range: SourceRange,
    pub checked_attr: bool,
}

/// Authenticated source rules after package inheritance and file overrides.
/// Omitted source settings inherit; the outermost defaults are both false.
/// Class overrides apply only to that declaration, not lexically nested ones.
/// Unsupported classes remain dynamic even when checking is requested.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedStrictPolicy {
    pub strict_assign: bool,
    pub checked_attr: bool,
    pub class_overrides: Vec<ClassPolicyOverride>,
}

impl ResolvedStrictPolicy {
    pub fn is_selected(&self) -> bool {
        self.strict_assign
            || self.checked_attr
            || self.class_overrides.iter().any(|rule| rule.checked_attr)
    }

    pub fn checked_attributes(&self, class_range: SourceRange) -> bool {
        self.class_overrides
            .iter()
            .find(|rule| rule.class_range == class_range)
            .map_or(self.checked_attr, |rule| rule.checked_attr)
    }

    pub fn checked_fields(&self, class_range: SourceRange) -> CheckedFieldPolicy {
        if self.checked_attributes(class_range) {
            CheckedFieldPolicy::SupportedAnnotations
        } else {
            CheckedFieldPolicy::Disabled
        }
    }

    pub fn fingerprint(&self) -> Result<Fingerprint, ContractError> {
        crate::artifact::canonical_bytes(self).map(Fingerprint::digest)
    }
}
