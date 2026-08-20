use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

use crate::ContractError;

/// The shared source identity used by profiles, IR, and authenticated facts.
/// Its historical FNV component remains unchanged; authenticated artifacts
/// additionally require the enclosing collision-resistant source digest.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Hash, PartialEq, Eq, Debug))]
pub struct ModuleContentId {
    pub module_name: String,
    pub source_hash: u64,
}

impl ModuleContentId {
    pub fn new(module_name: impl Into<String>, source_hash: u64) -> Self {
        Self {
            module_name: module_name.into(),
            source_hash,
        }
    }
}

/// A collision-resistant content fingerprint. Digests are not authority by
/// themselves; only a loader-trusted signature authenticates the manifest.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn digest(bytes: impl AsRef<[u8]>) -> Self {
        Self(Sha256::digest(bytes.as_ref()).into())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_hex(value: &str) -> Result<Self, ContractError> {
        decode_hex(value).map(Self)
    }

    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&encode_hex(&self.0))
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(de::Error::custom)
    }
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    output
}

pub(crate) fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], ContractError> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }

    if value.len() != N * 2 {
        return Err(ContractError::Encoding(format!(
            "expected {} lower-case hexadecimal digits",
            N * 2
        )));
    }
    let mut result = [0; N];
    for (output, pair) in result.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let high = digit(pair[0]);
        let low = digit(pair[1]);
        match (high, low) {
            (Some(high), Some(low)) => *output = (high << 4) | low,
            _ => {
                return Err(ContractError::Encoding(
                    "fingerprints and signatures require lower-case hexadecimal".into(),
                ));
            }
        }
    }
    Ok(result)
}

/// A deployment-selected immutable generation. Possessing this identifier
/// does not authenticate an artifact; the loader pins it independently.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[serde(transparent)]
pub struct ArtifactGenerationId(Fingerprint);

impl ArtifactGenerationId {
    pub const fn new(fingerprint: Fingerprint) -> Self {
        Self(fingerprint)
    }

    pub const fn fingerprint(self) -> Fingerprint {
        self.0
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    pub start: u32,
    pub end: u32,
}

impl SourceRange {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionKind {
    Module,
    Class,
    Function,
    Lambda,
    Assignment,
    Parameter,
    TypeAlias,
}

/// A lexical definition in original source bytes, never a Salsa identity.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    #[serde(with = "module_content_id_serde")]
    pub module: ModuleContentId,
    pub lexical_qualname: String,
    pub source_range: SourceRange,
    pub definition_kind: DefinitionKind,
}

impl SourceIdentity {
    pub fn module_body(module: ModuleContentId, source_size: u32) -> Self {
        Self {
            module,
            lexical_qualname: "<module>".into(),
            source_range: SourceRange::new(0, source_size),
            definition_kind: DefinitionKind::Module,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassReference {
    pub definition: SourceIdentity,
    pub source_digest: Fingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallExpressionKind {
    Call,
    AttributeCall,
    SuperCall,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSiteIdentity {
    #[serde(with = "module_content_id_serde")]
    pub module: ModuleContentId,
    pub source_digest: Fingerprint,
    /// The source execution owner. Module/class bodies retain their own
    /// lexical identities; no synthetic function definition is invented.
    pub enclosing_function: SourceIdentity,
    pub expression_range: SourceRange,
    pub expression_kind: CallExpressionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributeSiteIdentity {
    #[serde(with = "module_content_id_serde")]
    pub module: ModuleContentId,
    pub source_digest: Fingerprint,
    pub enclosing_function: SourceIdentity,
    pub expression_range: SourceRange,
}

/// The existing FNV-1a source identity, retained for SOAC interoperability.
/// Never use it instead of a collision-resistant authentication digest.
pub fn legacy_source_hash(source: &[u8]) -> u64 {
    source.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x00000100000001b3)
    })
}

/// Serde representation of the shared identity. The archive/profile layout
/// remains unchanged; enclosing authenticated artifacts carry SHA-256 too.
pub(crate) mod module_content_id_serde {
    use super::ModuleContentId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EncodedModuleId {
        module_name: String,
        source_hash: u64,
    }

    pub(crate) fn serialize<S: Serializer>(
        value: &ModuleContentId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        EncodedModuleId {
            module_name: value.module_name.clone(),
            source_hash: value.source_hash,
        }
        .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ModuleContentId, D::Error> {
        let value = EncodedModuleId::deserialize(deserializer)?;
        Ok(ModuleContentId::new(value.module_name, value.source_hash))
    }
}
