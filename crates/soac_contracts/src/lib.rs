//! Source-authenticated proposals for SOAC's strict language.
//!
//! This crate deliberately has no checker database, Python runtime, physical
//! layout, or native-code dependencies. Verification establishes where a
//! proposal came from and what source/configuration it describes. It does not
//! establish a runtime capability: the loader and the actual type/function
//! constructors must still install and seal the promised restrictions.

#![deny(unreachable_pub)]

mod artifact;
mod deployment;
mod diagnostics;
mod error;
mod facts;
mod identity;
mod policy;
mod validation;

pub use artifact::{
    ARTIFACT_SCHEMA_VERSION, ArtifactEnvironment, ArtifactExpectations, ArtifactSigningKey,
    ArtifactTrustAnchor, ArtifactVersions, CompleteArtifactGeneration, ConservativeAnalysis,
    DIALECT_VERSION, DependencyFingerprint, EncodedModuleShard, ModuleArtifactIndex, PythonVersion,
    STRICT_CONTRACT_VERSION, TypeArtifactManifest, VerifiedModuleTypeFacts,
    VerifiedTypeArtifactManifest, encode_module_shard, sign_manifest, verify_complete_generation,
    verify_manifest,
};
pub use deployment::{
    AnalysisDependency, AnalysisDependencySource, AnalysisDirectoryFilter,
    AnalysisDirectoryObservation, AnalysisEnvironmentVariable, AnalysisFileConfiguration,
    AnalysisInput, AnalysisInputState, DEPLOYMENT_SCHEMA_VERSION, DeployedModule,
    InterpreterIdentity, StrictArtifactDeployment, VerifiedAnalysisSnapshot,
    capture_analysis_input, capture_analysis_input_with_filters, verify_analysis_inputs,
};
pub use error::ContractError;
pub use facts::{
    AnnotationOrigin, AnnotationTarget, AttributeAccess, AttributeSiteFact, BaseReference,
    BuiltinType, CallBindingFact, CallSiteFact, CallUncertainty, CallableSignature,
    CallableTargetFact, ClassDictionarySemantics, ClassMemberFact, ClassMemberKind, ClassOpenness,
    ClassTransformFact, ClassTypeFact, DataclassOptions, DecoratorFact, DecoratorKind, DefaultFact,
    DescriptorFact, DescriptorKind, DiagnosticCode, DiagnosticScope, DiagnosticSeverity,
    DynamicClassReason, FieldKind, FieldReadPolicy, FieldReference, FieldTypeFact,
    FieldWritePolicy, FunctionKind, FunctionTypeFact, GeneratedFunctionFact, GlobalBindingFact,
    GlobalMutability, InheritanceFact, InitializationPolicy, LiteralValue, MetaclassFact,
    MethodBinding, MethodTypeFact, ModuleTypeFacts, NominalBindingFact, NominalBindingOwner,
    OverridePolicy, ParameterKind, ParameterTypeFact, ParticipationProposal, ProtocolFact,
    ReceiverTypeFact, SourceDialect, StaticType, StrictDiagnostic, TransformKind, TypeVariableFact,
    UncertaintyReason, UnsupportedReasonCode, UnsupportedTypeKind,
};
pub use identity::{
    ArchivedArtifactGenerationId, ArchivedDefinitionKind, ArchivedFingerprint,
    ArchivedModuleContentId, ArchivedSourceIdentity, ArchivedSourceRange, ArtifactGenerationId,
    AttributeSiteIdentity, CallExpressionKind, CallSiteIdentity, ClassReference, DefinitionKind,
    Fingerprint, ModuleContentId, SourceIdentity, SourceRange, legacy_source_hash,
};
pub use policy::{CheckedFieldPolicy, ClassPolicyOverride, ResolvedStrictPolicy};
pub use validation::validate_module_facts;

#[cfg(test)]
mod tests;
