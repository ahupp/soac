use soac_contracts::{
    ArtifactGenerationId, Fingerprint, ModuleContentId, SourceIdentity, VerifiedModuleTypeFacts,
};

/// Source-authentication metadata carried through every IR stage. This is not
/// a native capability: the runtime must retain and match its independently
/// verified artifact and actual code/module identities before using a plan.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StrictModuleSource {
    pub module: ModuleContentId,
    pub source_digest: Fingerprint,
    pub generation: ArtifactGenerationId,
    pub shard_digest: Fingerprint,
    pub cache_identity: Fingerprint,
}

impl StrictModuleSource {
    pub fn from_verified(facts: &VerifiedModuleTypeFacts) -> Self {
        Self {
            module: facts.facts().module.clone(),
            source_digest: facts.facts().source_digest,
            generation: facts.generation(),
            shard_digest: facts.shard_digest(),
            cache_identity: facts.cache_identity(),
        }
    }

    pub fn matches_verified(&self, facts: &VerifiedModuleTypeFacts) -> bool {
        self.module == facts.facts().module
            && self.source_digest == facts.facts().source_digest
            && self.generation == facts.generation()
            && self.shard_digest == facts.shard_digest()
            && self.cache_identity == facts.cache_identity()
    }
}

/// A semantic role assigned at the rewrite that creates the callable. Runtime
/// code must not rediscover class construction by guessing a generated name.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum CallableSourceRole {
    ModuleBody,
    SourceFunction,
    ClassNamespace,
    ClassConstruction,
    AnnotationProvider,
    TypeParameterScope,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CallableSourceOrigin {
    pub definition: SourceIdentity,
    pub role: CallableSourceRole,
}

/// Original-code exposure for one parser-owned generator expression. This
/// metadata selects the public gi_code/ag_code object, not source-function
/// admission, an execution code object, or a closure/binding capability.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GeneratorExpressionCode {
    /// Native expression span, including the actual call-argument parentheses
    /// when a sole generator argument does not have parentheses of its own.
    pub expression_range: soac_contracts::SourceRange,
    /// CPython's genexpr prologue is anchored to the original outer iterable,
    /// whose evaluation remains outside the suspended generator body.
    pub iterable_range: soac_contracts::SourceRange,
}
