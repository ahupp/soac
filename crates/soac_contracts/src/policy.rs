use serde::{Deserialize, Serialize};

use crate::{AnnotationOrigin, ContractError, Fingerprint, StaticType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticClassPolicy {
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedClassPolicy {
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypingFinalPolicy {
    Advisory,
    EnforceForParticipatingClasses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckedFieldPolicy {
    Disabled,
    SupportedAnnotations,
}

fn required_annotation_type(
    origin: AnnotationOrigin,
    value_type: &StaticType,
) -> Option<&StaticType> {
    (origin == AnnotationOrigin::Explicit && value_type.has_supported_value_shape())
        .then_some(value_type)
}

impl CheckedFieldPolicy {
    /// The explicit supported subset for field writes. Class participation or
    /// a function signature alone does not enable a field predicate.
    pub fn required_type(
        self,
        origin: AnnotationOrigin,
        value_type: &StaticType,
    ) -> Option<&StaticType> {
        (self == Self::SupportedAnnotations)
            .then(|| required_annotation_type(origin, value_type))
            .flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    TypeError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedValueTypePolicy {
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StdlibDataclassPolicy {
    Stdlib,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterPolicy {
    pub dataclasses: StdlibDataclassPolicy,
    pub pydantic: UnsupportedClassPolicy,
    pub django: UnsupportedClassPolicy,
    pub sqlalchemy: UnsupportedClassPolicy,
}

impl Default for AdapterPolicy {
    fn default() -> Self {
        Self {
            dataclasses: StdlibDataclassPolicy::Stdlib,
            pydantic: UnsupportedClassPolicy::Dynamic,
            django: UnsupportedClassPolicy::Dynamic,
            sqlalchemy: UnsupportedClassPolicy::Dynamic,
        }
    }
}

/// Effective policy after project selection and per-file overrides. There
/// are deliberately no per-class opt-in annotations or runtime revocation
/// switches. An enabled check is a language requirement, not a JIT guard.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedStrictPolicy {
    pub default_class_policy: AutomaticClassPolicy,
    pub unsupported_class_policy: UnsupportedClassPolicy,
    pub typing_final_policy: TypingFinalPolicy,
    pub checked_fields: CheckedFieldPolicy,
    pub field_failure: FailurePolicy,
    pub unsupported_value_type: UnsupportedValueTypePolicy,
    pub adapters: AdapterPolicy,
}

impl Default for ResolvedStrictPolicy {
    fn default() -> Self {
        Self {
            default_class_policy: AutomaticClassPolicy::Automatic,
            unsupported_class_policy: UnsupportedClassPolicy::Dynamic,
            typing_final_policy: TypingFinalPolicy::EnforceForParticipatingClasses,
            checked_fields: CheckedFieldPolicy::Disabled,
            field_failure: FailurePolicy::TypeError,
            unsupported_value_type: UnsupportedValueTypePolicy::Dynamic,
            adapters: AdapterPolicy::default(),
        }
    }
}

impl ResolvedStrictPolicy {
    pub fn fingerprint(&self) -> Result<Fingerprint, ContractError> {
        crate::artifact::canonical_bytes(self).map(Fingerprint::digest)
    }
}
