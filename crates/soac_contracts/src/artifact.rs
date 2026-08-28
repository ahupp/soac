use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::ModuleContentId;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::identity::{decode_hex, encode_hex, module_content_id_serde};
use crate::{
    ArtifactGenerationId, ContractError, Fingerprint, ModuleTypeFacts, ResolvedStrictPolicy,
    legacy_source_hash, validate_module_facts,
};

pub const ARTIFACT_SCHEMA_VERSION: u32 = 7;
pub const STRICT_CONTRACT_VERSION: u32 = 3;
pub const DIALECT_VERSION: u32 = 2;

const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
const MAX_SHARD_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const SIGNATURE_DOMAIN: &[u8] = b"SOAC-TYPE-CONTRACT-MANIFEST\0v6\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactVersions {
    pub schema_version: u32,
    pub strict_contract_version: u32,
    pub dialect_version: u32,
}

impl Default for ArtifactVersions {
    fn default() -> Self {
        Self {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            strict_contract_version: STRICT_CONTRACT_VERSION,
            dialect_version: DIALECT_VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonVersion {
    pub major: u8,
    pub minor: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConservativeAnalysis {
    pub strict_equality_semantics: bool,
    pub strict_generic_narrowing: bool,
}

impl Default for ConservativeAnalysis {
    fn default() -> Self {
        Self {
            strict_equality_semantics: true,
            strict_generic_narrowing: true,
        }
    }
}

/// Fully resolved offline analysis inputs. The checker-source fingerprint
/// binds the exact committed revision, tree, and checkout bytes.
/// These values are compared with loader-supplied expectations, never merely
/// compared with other files in the same writable artifact directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEnvironment {
    pub ty_revision: String,
    pub checker_source_fingerprint: Fingerprint,
    pub exporter_revision: String,
    pub python_version: PythonVersion,
    pub python_platform: String,
    pub cpython_abi_fingerprint: Fingerprint,
    pub normalized_project_policy: Fingerprint,
    pub resolved_typechecker_configuration: Fingerprint,
    pub import_search_path: Fingerprint,
    pub typeshed_fingerprint: Fingerprint,
    pub installed_stub_fingerprint: Fingerprint,
    pub installed_dependency_fingerprint: Fingerprint,
    pub analysis: ConservativeAnalysis,
}

impl ArtifactEnvironment {
    pub fn fingerprint(&self) -> Result<Fingerprint, ContractError> {
        canonical_bytes(self).map(Fingerprint::digest)
    }
}

/// A source/stub actually consumed by the checker. Installed distributions
/// and the resolver's complete search environment also participate in the
/// manifest environment. Fingerprinting never requires executing an import.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyFingerprint {
    #[serde(with = "module_content_id_serde")]
    pub module: ModuleContentId,
    pub source_digest: Fingerprint,
    pub source_size: u32,
    pub import_resolution: Fingerprint,
    pub effective_configuration: Fingerprint,
    pub strict_policy: Option<Fingerprint>,
    pub type_contract: Option<Fingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleArtifactIndex {
    #[serde(with = "module_content_id_serde")]
    pub module: ModuleContentId,
    pub source_digest: Fingerprint,
    pub source_size: u32,
    pub effective_policy: Fingerprint,
    pub shard_digest: Fingerprint,
    pub consumed_dependencies: Vec<DependencyFingerprint>,
}

impl ModuleArtifactIndex {
    pub fn from_shard(shard: &EncodedModuleShard) -> Result<Self, ContractError> {
        Ok(Self {
            module: shard.facts.module.clone(),
            source_digest: shard.facts.source_digest,
            source_size: shard.facts.source_size,
            effective_policy: shard.facts.language_policy.fingerprint()?,
            shard_digest: shard.digest,
            consumed_dependencies: shard.facts.consumed_dependencies.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeArtifactManifest {
    pub versions: ArtifactVersions,
    pub generation: ArtifactGenerationId,
    pub environment: ArtifactEnvironment,
    pub modules: Vec<ModuleArtifactIndex>,
}

impl TypeArtifactManifest {
    pub fn new(
        environment: ArtifactEnvironment,
        modules: Vec<ModuleArtifactIndex>,
    ) -> Result<Self, ContractError> {
        let mut manifest = Self {
            versions: ArtifactVersions::default(),
            generation: ArtifactGenerationId::new(Fingerprint::digest([])),
            environment,
            modules,
        };
        canonicalize_manifest(&mut manifest);
        manifest.generation = content_generation(&manifest)?;
        validate_manifest(&manifest)?;
        Ok(manifest)
    }
}

/// Loader-owned expectations. Do not populate these from the manifest
/// being verified, from Python-visible module attributes, or from a key file
/// inside the artifact directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactExpectations {
    pub generation: ArtifactGenerationId,
    pub environment: ArtifactEnvironment,
}

/// A build-side signing key; it is never serialized into an artifact.
pub struct ArtifactSigningKey(SigningKey);

impl ArtifactSigningKey {
    pub fn from_bytes(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    pub fn trust_anchor(&self) -> ArtifactTrustAnchor {
        ArtifactTrustAnchor(self.0.verifying_key())
    }
}

/// The runtime loader obtains this key from its trusted deployment boundary,
/// not from an artifact. A key identifier supplied by an artifact is not used
/// to choose authority.
#[derive(Clone)]
pub struct ArtifactTrustAnchor(VerifyingKey);

impl ArtifactTrustAnchor {
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, ContractError> {
        VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|_| ContractError::UntrustedSignature)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedManifestEnvelope {
    manifest: TypeArtifactManifest,
    signature: String,
}

/// Deterministically encoded source facts. This is still an unsigned
/// proposal; encoding or hashing it does not establish trust.
pub struct EncodedModuleShard {
    facts: ModuleTypeFacts,
    bytes: Vec<u8>,
    digest: Fingerprint,
}

impl EncodedModuleShard {
    pub fn facts(&self) -> &ModuleTypeFacts {
        &self.facts
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn digest(&self) -> Fingerprint {
        self.digest
    }

    pub fn file_name(&self) -> String {
        format!("{}.soac-types", self.digest)
    }
}

pub fn encode_module_shard(facts: &ModuleTypeFacts) -> Result<EncodedModuleShard, ContractError> {
    let facts = facts.canonicalized()?;
    validate_module_facts(&facts, None)?;
    let bytes = canonical_bytes(&facts)?;
    enforce_size(&bytes, "module shard", MAX_SHARD_BYTES)?;
    let digest = Fingerprint::digest(&bytes);
    Ok(EncodedModuleShard {
        facts,
        bytes,
        digest,
    })
}

pub fn sign_manifest(
    manifest: &TypeArtifactManifest,
    signing_key: &ArtifactSigningKey,
) -> Result<Vec<u8>, ContractError> {
    let mut manifest = manifest.clone();
    canonicalize_manifest(&mut manifest);
    validate_manifest(&manifest)?;
    let payload = signature_payload(&manifest)?;
    let signature = signing_key.0.sign(&payload);
    let bytes = canonical_bytes(&SignedManifestEnvelope {
        manifest,
        signature: encode_hex(&signature.to_bytes()),
    })?;
    enforce_size(&bytes, "manifest", MAX_MANIFEST_BYTES)?;
    Ok(bytes)
}

/// An authenticated manifest is not proof of a completely published
/// generation, nor of the current source/dependencies or runtime objects.
/// It can only be constructed by signature/generation/environment checking.
#[derive(Clone)]
pub struct VerifiedTypeArtifactManifest {
    manifest: Arc<TypeArtifactManifest>,
    manifest_digest: Fingerprint,
}

impl VerifiedTypeArtifactManifest {
    pub fn manifest(&self) -> &TypeArtifactManifest {
        &self.manifest
    }

    pub const fn manifest_digest(&self) -> Fingerprint {
        self.manifest_digest
    }

    pub fn module_index(&self, module_name: &str) -> Result<&ModuleArtifactIndex, ContractError> {
        self.manifest
            .modules
            .binary_search_by(|entry| entry.module.module_name.as_str().cmp(module_name))
            .map(|index| &self.manifest.modules[index])
            .map_err(|_| ContractError::MissingModule(module_name.into()))
    }

    /// Authenticate one immutable shard against this generation and the
    /// loader's actual source, effective policy, and consumed dependencies.
    /// No Python modules or annotation providers are executed by this API.
    pub fn verify_module(
        &self,
        module_name: &str,
        source: &[u8],
        expected_policy: &ResolvedStrictPolicy,
        current_dependencies: &[DependencyFingerprint],
        shard_bytes: &[u8],
    ) -> Result<VerifiedModuleTypeFacts, ContractError> {
        let index = self.module_index(module_name)?;
        if index.source_digest != Fingerprint::digest(source)
            || index.module.source_hash != legacy_source_hash(source)
            || usize::try_from(index.source_size).ok() != Some(source.len())
        {
            return Err(ContractError::SourceMismatch(module_name.into()));
        }
        if index.effective_policy != expected_policy.fingerprint()? {
            return Err(ContractError::PolicyMismatch(module_name.into()));
        }
        let mut current_dependencies = current_dependencies.to_vec();
        canonicalize_dependencies(&mut current_dependencies);
        validate_dependencies(&current_dependencies, module_name)?;
        if index.consumed_dependencies != current_dependencies {
            return Err(ContractError::DependencyMismatch(module_name.into()));
        }
        let facts = self.verify_shard(index, shard_bytes)?;
        if facts.language_policy != *expected_policy {
            return Err(ContractError::PolicyMismatch(module_name.into()));
        }
        validate_module_facts(&facts, Some(source))?;
        let cache_identity = Fingerprint::digest(canonical_bytes(&(
            self.manifest_digest,
            self.manifest.generation,
            index.shard_digest,
            self.manifest.environment.fingerprint()?,
        ))?);
        Ok(VerifiedModuleTypeFacts {
            facts: Arc::new(facts),
            generation: self.manifest.generation,
            shard_digest: index.shard_digest,
            cache_identity,
        })
    }

    fn verify_shard(
        &self,
        index: &ModuleArtifactIndex,
        shard_bytes: &[u8],
    ) -> Result<ModuleTypeFacts, ContractError> {
        enforce_size(shard_bytes, "module shard", MAX_SHARD_BYTES)?;
        if Fingerprint::digest(shard_bytes) != index.shard_digest {
            return Err(ContractError::ShardMismatch(
                index.module.module_name.clone(),
            ));
        }
        let facts: ModuleTypeFacts = decode_canonical(shard_bytes)?;
        validate_module_facts(&facts, None)?;
        if facts != facts.canonicalized()? {
            return Err(ContractError::NonCanonicalEncoding);
        }
        if facts.module != index.module
            || facts.source_digest != index.source_digest
            || facts.source_size != index.source_size
            || facts.language_policy.fingerprint()? != index.effective_policy
            || facts.consumed_dependencies != index.consumed_dependencies
        {
            return Err(ContractError::ShardMismatch(
                index.module.module_name.clone(),
            ));
        }
        Ok(facts)
    }
}

pub fn verify_manifest(
    bytes: &[u8],
    trust_anchor: &ArtifactTrustAnchor,
    expected: &ArtifactExpectations,
) -> Result<VerifiedTypeArtifactManifest, ContractError> {
    enforce_size(bytes, "manifest", MAX_MANIFEST_BYTES)?;
    let envelope: SignedManifestEnvelope = decode_canonical(bytes)?;
    let signature = Signature::from_bytes(&decode_hex::<64>(&envelope.signature)?);
    trust_anchor
        .0
        .verify_strict(&signature_payload(&envelope.manifest)?, &signature)
        .map_err(|_| ContractError::UntrustedSignature)?;
    validate_manifest(&envelope.manifest)?;
    let mut canonical = envelope.manifest.clone();
    canonicalize_manifest(&mut canonical);
    if canonical != envelope.manifest {
        return Err(ContractError::NonCanonicalEncoding);
    }
    if envelope.manifest.generation != expected.generation {
        return Err(ContractError::GenerationMismatch);
    }
    compare_environment(&envelope.manifest.environment, &expected.environment)?;
    Ok(VerifiedTypeArtifactManifest {
        manifest: Arc::new(envelope.manifest),
        manifest_digest: Fingerprint::digest(bytes),
    })
}

/// A proof that every shard in a signed generation was present and matched
/// its index during verification. Shards are not retained in memory, so this
/// is suitable for a large immutable deployment snapshot. Every subsequent
/// module load must still call `verify_module`, which rechecks the bytes;
/// replacing a file after this preflight cannot bypass authentication.
pub struct CompleteArtifactGeneration {
    manifest: VerifiedTypeArtifactManifest,
}

impl CompleteArtifactGeneration {
    pub fn manifest(&self) -> &VerifiedTypeArtifactManifest {
        &self.manifest
    }
}

pub fn verify_complete_generation(
    manifest: VerifiedTypeArtifactManifest,
    mut read_shard: impl FnMut(Fingerprint) -> Result<Vec<u8>, ContractError>,
) -> Result<CompleteArtifactGeneration, ContractError> {
    for index in &manifest.manifest.modules {
        let bytes = read_shard(index.shard_digest)?;
        manifest.verify_shard(index, &bytes)?;
    }
    Ok(CompleteArtifactGeneration { manifest })
}

/// An authenticated source-bound proposal, not a runtime capability.
/// Fields are private, it cannot be deserialized, and callers receive only
/// immutable facts. Runtime sealing must establish each independent fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedModuleTypeFacts {
    facts: Arc<ModuleTypeFacts>,
    generation: ArtifactGenerationId,
    shard_digest: Fingerprint,
    cache_identity: Fingerprint,
}

impl VerifiedModuleTypeFacts {
    pub fn facts(&self) -> &ModuleTypeFacts {
        &self.facts
    }

    pub const fn generation(&self) -> ArtifactGenerationId {
        self.generation
    }

    pub const fn shard_digest(&self) -> Fingerprint {
        self.shard_digest
    }

    pub const fn cache_identity(&self) -> Fingerprint {
        self.cache_identity
    }
}

fn signature_payload(manifest: &TypeArtifactManifest) -> Result<Vec<u8>, ContractError> {
    let manifest = canonical_bytes(manifest)?;
    let mut payload = Vec::with_capacity(SIGNATURE_DOMAIN.len() + manifest.len());
    payload.extend_from_slice(SIGNATURE_DOMAIN);
    payload.extend_from_slice(&manifest);
    Ok(payload)
}

fn content_generation(
    manifest: &TypeArtifactManifest,
) -> Result<ArtifactGenerationId, ContractError> {
    let mut bytes = b"SOAC-TYPE-CONTRACT-GENERATION\0v1\0".to_vec();
    bytes.extend_from_slice(&canonical_bytes(&(
        manifest.versions,
        &manifest.environment,
        &manifest.modules,
    ))?);
    Ok(ArtifactGenerationId::new(Fingerprint::digest(bytes)))
}

pub(crate) fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    fn sort_objects(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                // Explicitly sort even if another crate enabled serde_json's
                // preserve_order feature through Cargo feature unification.
                object.sort_keys();
                for value in object.values_mut() {
                    sort_objects(value);
                }
            }
            serde_json::Value::Array(array) => {
                for value in array {
                    sort_objects(value);
                }
            }
            _ => {}
        }
    }

    let mut value =
        serde_json::to_value(value).map_err(|error| ContractError::Encoding(error.to_string()))?;
    sort_objects(&mut value);
    serde_json::to_vec(&value).map_err(|error| ContractError::Encoding(error.to_string()))
}

fn decode_canonical<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, ContractError> {
    let value: T = serde_json::from_slice(bytes)
        .map_err(|error| ContractError::Encoding(error.to_string()))?;
    // This also rejects duplicate map/set members, alternate number/string
    // encodings, trailing bytes and unrecognized fields rather than assigning
    // different logical meanings to the same signed payload.
    if canonical_bytes(&value)? != bytes {
        return Err(ContractError::NonCanonicalEncoding);
    }
    Ok(value)
}

fn enforce_size(bytes: &[u8], kind: &'static str, limit: usize) -> Result<(), ContractError> {
    if bytes.len() > limit {
        Err(ContractError::SizeLimit { kind, limit })
    } else {
        Ok(())
    }
}

fn canonicalize_manifest(manifest: &mut TypeArtifactManifest) {
    manifest
        .modules
        .sort_by(|left, right| left.module.module_name.cmp(&right.module.module_name));
    for index in &mut manifest.modules {
        canonicalize_dependencies(&mut index.consumed_dependencies);
    }
}

pub(crate) fn canonicalize_dependencies(dependencies: &mut [DependencyFingerprint]) {
    dependencies.sort_by(|left, right| left.module.module_name.cmp(&right.module.module_name));
}

pub(crate) fn validate_dependencies(
    dependencies: &[DependencyFingerprint],
    owner: &str,
) -> Result<(), ContractError> {
    let mut names = BTreeSet::new();
    for dependency in dependencies {
        crate::validation::validate_module_name(&dependency.module.module_name)?;
        if dependency.module.module_name == owner {
            return Err(ContractError::InvalidStructure(
                "a shard must not list itself as an external dependency".into(),
            ));
        }
        if !names.insert(&dependency.module.module_name) {
            return Err(ContractError::DependencyMismatch(
                dependency.module.module_name.clone(),
            ));
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &TypeArtifactManifest) -> Result<(), ContractError> {
    for (kind, found, expected) in [
        (
            "artifact schema",
            manifest.versions.schema_version,
            ARTIFACT_SCHEMA_VERSION,
        ),
        (
            "strict contract",
            manifest.versions.strict_contract_version,
            STRICT_CONTRACT_VERSION,
        ),
        (
            "strict dialect",
            manifest.versions.dialect_version,
            DIALECT_VERSION,
        ),
    ] {
        if found != expected {
            return Err(ContractError::VersionMismatch {
                kind,
                found,
                expected,
            });
        }
    }
    validate_environment(&manifest.environment)?;
    let mut modules = BTreeMap::new();
    for index in &manifest.modules {
        crate::validation::validate_module_name(&index.module.module_name)?;
        if modules.insert(&index.module.module_name, index).is_some() {
            return Err(ContractError::InvalidStructure(format!(
                "duplicate manifest module {}",
                index.module.module_name
            )));
        }
        validate_dependencies(&index.consumed_dependencies, &index.module.module_name)?;
    }
    let mut consumed = BTreeMap::<&str, DependencyFingerprint>::new();
    for index in &manifest.modules {
        for dependency in &index.consumed_dependencies {
            if let Some(previous) = consumed.get_mut(dependency.module.module_name.as_str()) {
                if previous.module != dependency.module
                    || previous.source_digest != dependency.source_digest
                    || previous.source_size != dependency.source_size
                    || previous.import_resolution != dependency.import_resolution
                    || previous.effective_configuration != dependency.effective_configuration
                    || previous
                        .strict_policy
                        .zip(dependency.strict_policy)
                        .is_some_and(|(left, right)| left != right)
                    || previous
                        .type_contract
                        .zip(dependency.type_contract)
                        .is_some_and(|(left, right)| left != right)
                {
                    return Err(ContractError::DependencyMismatch(
                        dependency.module.module_name.clone(),
                    ));
                }
                previous.strict_policy = previous.strict_policy.or(dependency.strict_policy);
                previous.type_contract = previous.type_contract.or(dependency.type_contract);
            } else {
                consumed.insert(&dependency.module.module_name, dependency.clone());
            }
            if let Some(producer) = modules.get(&dependency.module.module_name) {
                if producer.module != dependency.module
                    || producer.source_digest != dependency.source_digest
                    || producer.source_size != dependency.source_size
                    || dependency
                        .type_contract
                        .is_some_and(|digest| digest != producer.shard_digest)
                    || dependency
                        .strict_policy
                        .is_some_and(|policy| policy != producer.effective_policy)
                {
                    return Err(ContractError::DependencyMismatch(
                        dependency.module.module_name.clone(),
                    ));
                }
            }
        }
    }
    if content_generation(manifest)? != manifest.generation {
        return Err(ContractError::GenerationMismatch);
    }
    Ok(())
}

fn validate_environment(environment: &ArtifactEnvironment) -> Result<(), ContractError> {
    for (name, value) in [
        ("ty revision", environment.ty_revision.as_str()),
        ("exporter revision", environment.exporter_revision.as_str()),
        ("Python platform", environment.python_platform.as_str()),
    ] {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(ContractError::InvalidStructure(format!("invalid {name}")));
        }
    }
    if environment.python_version.major != 3 || environment.python_version.minor < 7 {
        return Err(ContractError::InvalidStructure(
            "invalid Python version".into(),
        ));
    }
    if !environment.analysis.strict_equality_semantics
        || !environment.analysis.strict_generic_narrowing
    {
        return Err(ContractError::InvalidPolicy(
            "type-fact production requires conservative equality and generic narrowing".into(),
        ));
    }
    Ok(())
}

fn compare_environment(
    actual: &ArtifactEnvironment,
    expected: &ArtifactEnvironment,
) -> Result<(), ContractError> {
    validate_environment(expected)?;
    macro_rules! compare {
        ($($field:ident),+ $(,)?) => {
            $(if actual.$field != expected.$field {
                return Err(ContractError::EnvironmentMismatch(stringify!($field)));
            })+
        };
    }
    compare!(
        ty_revision,
        checker_source_fingerprint,
        exporter_revision,
        python_version,
        python_platform,
        cpython_abi_fingerprint,
        normalized_project_policy,
        resolved_typechecker_configuration,
        import_search_path,
        typeshed_fingerprint,
        installed_stub_fingerprint,
        installed_dependency_fingerprint,
        analysis,
    );
    Ok(())
}
