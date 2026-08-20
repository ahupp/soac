use thiserror::Error;

/// Failure to authenticate or structurally validate an offline proposal.
#[derive(Debug, Error)]
pub enum ContractError {
    #[error("invalid contract encoding: {0}")]
    Encoding(String),
    #[error("contract exceeds the {kind} byte limit ({limit})")]
    SizeLimit { kind: &'static str, limit: usize },
    #[error("contract encoding is not canonical")]
    NonCanonicalEncoding,
    #[error("unsupported {kind} version: expected {expected}, found {found}")]
    VersionMismatch {
        kind: &'static str,
        expected: u32,
        found: u32,
    },
    #[error("manifest signature is not trusted")]
    UntrustedSignature,
    #[error("artifact generation does not match the loader's pinned generation")]
    GenerationMismatch,
    #[error("artifact environment does not match the loader's {0}")]
    EnvironmentMismatch(&'static str),
    #[error("invalid strict policy: {0}")]
    InvalidPolicy(String),
    #[error("invalid contract structure: {0}")]
    InvalidStructure(String),
    #[error("invalid source identity: {0}")]
    InvalidSourceIdentity(String),
    #[error("invalid static type: {0}")]
    InvalidType(String),
    #[error("blocking strict diagnostic: {0}")]
    BlockingDiagnostic(String),
    #[error("module is not present in the signed manifest: {0}")]
    MissingModule(String),
    #[error("missing module shard: {0}")]
    MissingShard(String),
    #[error("source does not match the authenticated contract for {0}")]
    SourceMismatch(String),
    #[error("effective strict policy does not match the contract for {0}")]
    PolicyMismatch(String),
    #[error("consumed dependency does not match the contract: {0}")]
    DependencyMismatch(String),
    #[error("module shard does not match its signed index: {0}")]
    ShardMismatch(String),
}
