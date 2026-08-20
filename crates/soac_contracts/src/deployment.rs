//! The loader's out-of-band deployment configuration and observed analysis inputs.
//!
//! These are ordinary data, not proofs. A deployment descriptor must be supplied
//! by the trusted process startup boundary, separately from the writable artifact
//! directory. Deserializing one never authenticates a manifest or a runtime object.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactEnvironment, ArtifactGenerationId, ArtifactTrustAnchor, ConservativeAnalysis,
    ContractError, DependencyFingerprint, Fingerprint, ModuleContentId, PythonVersion,
    ResolvedStrictPolicy, legacy_source_hash,
};

pub const DEPLOYMENT_SCHEMA_VERSION: u32 = 2;

/// The part of a directory listing actually consumed by offline analysis.
/// A source-selection view is not an import-resolution exclusion: imports use
/// independent name, prefix, suffix, or complete-listing observations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisDirectoryFilter {
    All,
    Name { name: String },
    Prefix { prefix: String },
    Suffix { suffix: String },
    SourceSelection { excluded_names: Vec<String> },
}

impl AnalysisDirectoryFilter {
    pub fn includes(
        &self,
        name: &str,
        is_file: bool,
        is_directory: bool,
        is_symlink: bool,
    ) -> bool {
        match self {
            Self::All => true,
            Self::Name { name: selected } => name == selected,
            Self::Prefix { prefix } => name.starts_with(prefix),
            Self::Suffix { suffix } => name.ends_with(suffix),
            Self::SourceSelection { excluded_names } => {
                !excluded_names.iter().any(|excluded| excluded == name)
                    && (is_directory || is_symlink || (is_file && name.ends_with(".py")))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisDirectoryObservation {
    pub filter: AnalysisDirectoryFilter,
    pub entries: Fingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisInputState {
    Missing,
    File {
        canonical_path: PathBuf,
        digest: Fingerprint,
        size: u64,
    },
    Directory {
        canonical_path: PathBuf,
        /// Empty for existence-only observations. Every view was also supplied
        /// to the semantic consumer; revalidation applies the identical filter.
        observations: Vec<AnalysisDirectoryObservation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisInput {
    pub path: PathBuf,
    pub state: AnalysisInputState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisEnvironmentVariable {
    pub name: String,
    pub value: Option<Fingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployedModule {
    pub module_name: String,
    pub source_path: PathBuf,
    pub policy: ResolvedStrictPolicy,
}

/// The actual per-file checker settings captured by the offline driver after
/// project, script, and path overrides. The surrounding configuration files are
/// separately observed. These are startup-pinned expectations, not settings
/// recovered from an artifact or re-inferred by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFileConfiguration {
    pub python_version: PythonVersion,
    pub python_platform: String,
    pub analysis: ConservativeAnalysis,
    pub respect_type_ignore_comments: bool,
    /// Resolver order matters; do not sort this list.
    pub import_search_paths: Vec<String>,
    pub enabled_diagnostics: BTreeMap<String, String>,
}

impl AnalysisFileConfiguration {
    pub fn fingerprint(
        &self,
        environment: &ArtifactEnvironment,
    ) -> Result<Fingerprint, ContractError> {
        if self.python_version != environment.python_version
            || self.python_platform != environment.python_platform
            || self.analysis != ConservativeAnalysis::default()
            || self.analysis != environment.analysis
            || self.enabled_diagnostics.iter().any(|(name, level)| {
                name.is_empty() || !matches!(level.as_str(), "info" | "warning" | "error" | "fatal")
            })
        {
            return Err(ContractError::InvalidStructure(
                "dependency analysis policy does not match the authenticated environment".into(),
            ));
        }
        for path in &self.import_search_paths {
            absolute(Path::new(path))?;
        }
        Ok(Fingerprint::digest(crate::artifact::canonical_bytes(&(
            "SOAC-ANALYSIS-FILE-CONFIGURATION-v1",
            environment.resolved_typechecker_configuration,
            self,
        ))?))
    }
}

/// File identity owned by the loader, without checker database IDs. Vendored
/// sources are bound to the exact checker/typeshed build in the startup
/// environment; they are never interpreted as filesystem paths at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisDependencySource {
    System { path: PathBuf },
    Vendored { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisDependency {
    pub importer_module: String,
    #[serde(with = "crate::identity::module_content_id_serde")]
    pub module: ModuleContentId,
    pub source: AnalysisDependencySource,
    pub source_digest: Fingerprint,
    pub source_size: u32,
    pub configuration: AnalysisFileConfiguration,
}

impl AnalysisDependency {
    /// Derive an expected manifest dependency from out-of-band observations.
    /// This checks record consistency, not current disk bytes. Native consumers
    /// use `StrictArtifactDeployment::verified_analysis_dependencies` instead.
    pub fn fingerprint(
        &self,
        environment: &ArtifactEnvironment,
        modules: &[DeployedModule],
        inputs: &[AnalysisInput],
    ) -> Result<DependencyFingerprint, ContractError> {
        crate::validation::validate_module_name(&self.importer_module)?;
        crate::validation::validate_module_name(&self.module.module_name)?;
        if self.importer_module == self.module.module_name
            || !modules
                .iter()
                .any(|module| module.module_name == self.importer_module)
        {
            return Err(ContractError::DependencyMismatch(
                self.importer_module.clone(),
            ));
        }
        let deployed = modules
            .iter()
            .find(|module| module.module_name == self.module.module_name);
        let (import_resolution, strict_policy) = match &self.source {
            AnalysisDependencySource::System { path } => {
                absolute(path)?;
                let Some(AnalysisInputState::File {
                    canonical_path,
                    digest,
                    size,
                }) = inputs
                    .iter()
                    .find(|input| &input.path == path)
                    .map(|input| &input.state)
                else {
                    return Err(ContractError::DependencyMismatch(
                        self.module.module_name.clone(),
                    ));
                };
                if *digest != self.source_digest
                    || *size != u64::from(self.source_size)
                    || deployed.is_some_and(|module| &module.source_path != canonical_path)
                {
                    return Err(ContractError::DependencyMismatch(
                        self.module.module_name.clone(),
                    ));
                }
                (
                    Fingerprint::digest(crate::artifact::canonical_bytes(&(
                        "SOAC-SYSTEM-DEPENDENCY-v1",
                        path,
                    ))?),
                    deployed
                        .map(|module| module.policy.fingerprint())
                        .transpose()?,
                )
            }
            AnalysisDependencySource::Vendored { path } => {
                if path.is_empty()
                    || path.contains(['\\', '\0'])
                    || path
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                    || deployed.is_some()
                {
                    return Err(ContractError::DependencyMismatch(
                        self.module.module_name.clone(),
                    ));
                }
                (
                    Fingerprint::digest(crate::artifact::canonical_bytes(&(
                        "SOAC-VENDORED-DEPENDENCY-v1",
                        environment.checker_source_fingerprint,
                        environment.typeshed_fingerprint,
                        path,
                        &self.module,
                        self.source_digest,
                        self.source_size,
                    ))?),
                    None,
                )
            }
        };
        Ok(DependencyFingerprint {
            module: self.module.clone(),
            source_digest: self.source_digest,
            source_size: self.source_size,
            import_resolution,
            effective_configuration: self.configuration.fingerprint(environment)?,
            strict_policy,
            // The offline checker consumes source, not a previously issued
            // native type contract. Runtime capabilities are established later.
            type_contract: None,
        })
    }
}

/// Owned results of the build-side isolated interpreter probe. This record is
/// expected startup metadata, not evidence of the currently running process.
/// Native startup must independently compare its executable, mapped library,
/// version/platform, and actual file bytes before relying on this expectation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterpreterIdentity {
    pub version: [u8; 2],
    pub platform: String,
    pub prefix: String,
    pub executable: String,
    pub build_directory: String,
    pub site_packages: Vec<String>,
    pub real_stdlib: String,
    pub abi_files: Vec<String>,
    pub configuration_files: Vec<String>,
    pub configuration: BTreeMap<String, serde_json::Value>,
}

impl InterpreterIdentity {
    pub fn abi_fingerprint(&self, inputs: &[AnalysisInput]) -> Result<Fingerprint, ContractError> {
        for path in [
            &self.prefix,
            &self.executable,
            &self.build_directory,
            &self.real_stdlib,
        ]
        .into_iter()
        .chain(&self.site_packages)
        .chain(&self.abi_files)
        .chain(&self.configuration_files)
        {
            absolute(Path::new(path))?;
        }
        if !self.abi_files.windows(2).all(|pair| pair[0] < pair[1])
            || !self.abi_files.contains(&self.executable)
        {
            return Err(ContractError::InvalidStructure(
                "target ABI files must be sorted, unique, and include the executable".into(),
            ));
        }
        let mut abi_inputs = Vec::new();
        for path in &self.abi_files {
            let input = inputs
                .iter()
                .find(|input| input.path == Path::new(path))
                .ok_or_else(|| {
                    ContractError::InvalidStructure(
                        "target ABI file is absent from observed inputs".into(),
                    )
                })?;
            if !matches!(input.state, AnalysisInputState::File { size, .. } if size > 0) {
                return Err(ContractError::InvalidStructure(
                    "target ABI input must be a nonempty file".into(),
                ));
            }
            abi_inputs.push(input);
        }
        for path in &self.configuration_files {
            if !inputs.iter().any(|input| input.path == Path::new(path)) {
                return Err(ContractError::InvalidStructure(
                    "target interpreter configuration is not an observed input".into(),
                ));
            }
        }
        abi_inputs.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(Fingerprint::digest(crate::artifact::canonical_bytes(&(
            self, abi_inputs,
        ))?))
    }
}

/// Startup authority, not an artifact-provided key or an inferred expectation.
/// The loader must capture this before running application code and may not
/// accept a replacement from Python-visible module state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrictArtifactDeployment {
    pub schema_version: u32,
    pub artifact_directory: PathBuf,
    pub generation: ArtifactGenerationId,
    pub environment: ArtifactEnvironment,
    pub target_interpreter: InterpreterIdentity,
    pub trust_anchor: [u8; 32],
    pub modules: Vec<DeployedModule>,
    pub analysis_dependencies: Vec<AnalysisDependency>,
    pub analysis_inputs: Vec<AnalysisInput>,
    pub analysis_environment: Vec<AnalysisEnvironmentVariable>,
}

/// Point-in-time dependency observations for one callback-free loader
/// construction. This is not a persistent filesystem snapshot or permission
/// to skip fresh verification when a module is later admitted.
///
/// Private fields prevent manufacturing observations from manifest claims;
/// borrowed consumer names tie them to the startup descriptor that was
/// independently verified. Native consumers must discard this value before
/// publishing the loader or invoking Python callbacks.
#[derive(Debug)]
pub struct VerifiedAnalysisSnapshot<'deployment> {
    dependencies: BTreeMap<&'deployment str, Vec<DependencyFingerprint>>,
}

impl VerifiedAnalysisSnapshot<'_> {
    pub fn dependencies(&self, importer: &str) -> Result<&[DependencyFingerprint], ContractError> {
        self.dependencies
            .get(importer)
            .map(Vec::as_slice)
            .ok_or_else(|| ContractError::DependencyMismatch(importer.into()))
    }
}

struct ObservedDependencySource {
    size: u64,
    digest: Fingerprint,
    source_hash: u64,
}

impl ObservedDependencySource {
    fn read(path: &Path, size: u32) -> Result<Self, ContractError> {
        let bytes = read_file_bounded(path, u64::from(size))?;
        Ok(Self {
            size: bytes.len() as u64,
            digest: Fingerprint::digest(&bytes),
            source_hash: legacy_source_hash(&bytes),
        })
    }

    fn verify(&self, dependency: &AnalysisDependency) -> Result<(), ContractError> {
        if self.size != u64::from(dependency.source_size)
            || self.digest != dependency.source_digest
            || self.source_hash != dependency.module.source_hash
        {
            return Err(ContractError::DependencyMismatch(
                dependency.module.module_name.clone(),
            ));
        }
        Ok(())
    }
}

impl StrictArtifactDeployment {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != DEPLOYMENT_SCHEMA_VERSION {
            return Err(ContractError::InvalidStructure(
                "unsupported strict deployment schema".into(),
            ));
        }
        ArtifactTrustAnchor::from_bytes(&self.trust_anchor)?;
        if self.target_interpreter.version
            != [
                self.environment.python_version.major,
                self.environment.python_version.minor,
            ]
            || self.target_interpreter.platform != self.environment.python_platform
            || self
                .target_interpreter
                .abi_fingerprint(&self.analysis_inputs)?
                != self.environment.cpython_abi_fingerprint
        {
            return Err(ContractError::InvalidStructure(
                "deployment interpreter does not match its ABI environment".into(),
            ));
        }
        absolute(&self.artifact_directory)?;
        let mut modules = BTreeSet::new();
        for module in &self.modules {
            crate::validation::validate_module_name(&module.module_name)?;
            absolute(&module.source_path)?;
            if !modules.insert(&module.module_name) {
                return Err(ContractError::InvalidStructure(
                    "duplicate deployment module identity".into(),
                ));
            }
        }
        let mut inputs = BTreeSet::new();
        for input in &self.analysis_inputs {
            absolute(&input.path)?;
            if !inputs.insert(&input.path) {
                return Err(ContractError::InvalidStructure(
                    "duplicate deployment analysis input".into(),
                ));
            }
            match &input.state {
                AnalysisInputState::File { canonical_path, .. } => absolute(canonical_path)?,
                AnalysisInputState::Directory {
                    canonical_path,
                    observations,
                } => {
                    absolute(canonical_path)?;
                    let mut previous = None;
                    for observation in observations {
                        if previous.is_some_and(|previous| previous >= &observation.filter) {
                            return Err(ContractError::InvalidStructure(
                                "directory observations must be sorted and unique".into(),
                            ));
                        }
                        if let AnalysisDirectoryFilter::SourceSelection { excluded_names } =
                            &observation.filter
                        {
                            if !excluded_names.windows(2).all(|pair| pair[0] < pair[1])
                                || excluded_names.iter().any(|name| {
                                    name.is_empty()
                                        || name == "."
                                        || name == ".."
                                        || name.contains(['/', '\\', '\0'])
                                })
                            {
                                return Err(ContractError::InvalidStructure("source-selection exclusions must be sorted directory entry names".into()));
                            }
                        }
                        previous = Some(&observation.filter);
                    }
                }
                AnalysisInputState::Missing => {}
            }
        }
        let mut variables = BTreeSet::new();
        for variable in &self.analysis_environment {
            if variable.name.is_empty()
                || variable.name.contains(['\0', '='])
                || !variables.insert(&variable.name)
            {
                return Err(ContractError::InvalidStructure(
                    "invalid or duplicate analysis environment variable".into(),
                ));
            }
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &self.analysis_dependencies {
            if !dependencies.insert((&dependency.importer_module, &dependency.module.module_name)) {
                return Err(ContractError::DependencyMismatch(
                    dependency.module.module_name.clone(),
                ));
            }
            dependency.fingerprint(&self.environment, &self.modules, &self.analysis_inputs)?;
        }
        Ok(())
    }

    /// Verify the complete analysis input set once and independently rebuild
    /// every selected consumer's dependency expectations. Only System file
    /// observations are shared; consumer/configuration/source-role domains
    /// are reconstructed separately for every dependency record.
    ///
    /// Use only during a callback-free loader construction and discard the
    /// result before publication. Later admission must call
    /// `verified_analysis_dependencies` again to observe current bytes.
    pub fn verified_analysis_snapshot(
        &self,
    ) -> Result<VerifiedAnalysisSnapshot<'_>, ContractError> {
        self.validate()?;
        verify_analysis_inputs(&self.analysis_inputs)?;
        let mut dependencies: BTreeMap<_, Vec<_>> = self
            .modules
            .iter()
            .map(|module| (module.module_name.as_str(), Vec::new()))
            .collect();
        let mut sources = BTreeMap::new();
        for dependency in &self.analysis_dependencies {
            if let AnalysisDependencySource::System { path } = &dependency.source {
                let observed = match sources.entry(path.as_path()) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                        ObservedDependencySource::read(path, dependency.source_size)?,
                    ),
                };
                // Sharing actual bytes must never share a consumer's claimed
                // source hash, role, policy, or effective configuration.
                observed.verify(dependency)?;
            }
            let fingerprints = dependencies
                .get_mut(dependency.importer_module.as_str())
                .ok_or_else(|| {
                    ContractError::DependencyMismatch(dependency.importer_module.clone())
                })?;
            fingerprints.push(dependency.fingerprint(
                &self.environment,
                &self.modules,
                &self.analysis_inputs,
            )?);
        }
        for fingerprints in dependencies.values_mut() {
            crate::artifact::canonicalize_dependencies(fingerprints);
        }
        Ok(VerifiedAnalysisSnapshot { dependencies })
    }

    /// Independently rebuild one consumer's dependency expectations. The
    /// caller must capture this descriptor at trusted startup and separately
    /// verify native interpreter identity. No manifest or shard data is used.
    pub fn verified_analysis_dependencies(
        &self,
        importer: &str,
    ) -> Result<Vec<DependencyFingerprint>, ContractError> {
        self.validate()?;
        if !self
            .modules
            .iter()
            .any(|module| module.module_name == importer)
        {
            return Err(ContractError::DependencyMismatch(importer.into()));
        }
        verify_analysis_inputs(&self.analysis_inputs)?;
        let mut fingerprints = Vec::new();
        for dependency in self
            .analysis_dependencies
            .iter()
            .filter(|dependency| dependency.importer_module == importer)
        {
            if let AnalysisDependencySource::System { path } = &dependency.source {
                let bytes = read_file_bounded(path, u64::from(dependency.source_size))?;
                if Fingerprint::digest(&bytes) != dependency.source_digest
                    || legacy_source_hash(&bytes) != dependency.module.source_hash
                {
                    return Err(ContractError::DependencyMismatch(
                        dependency.module.module_name.clone(),
                    ));
                }
            }
            fingerprints.push(dependency.fingerprint(
                &self.environment,
                &self.modules,
                &self.analysis_inputs,
            )?);
        }
        crate::artifact::canonicalize_dependencies(&mut fingerprints);
        Ok(fingerprints)
    }
}

fn absolute(path: &Path) -> Result<(), ContractError> {
    if !path.is_absolute() || path.to_str().is_none() {
        return Err(ContractError::InvalidStructure(
            "deployment paths must be absolute UTF-8 paths".into(),
        ));
    }
    Ok(())
}

fn io_error(path: &Path, error: std::io::Error) -> ContractError {
    ContractError::InvalidStructure(format!(
        "cannot read analysis input {}: {error}",
        path.display()
    ))
}

fn read_file_bounded(path: &Path, size: u64) -> Result<Vec<u8>, ContractError> {
    let file = fs::File::open(path).map_err(|error| io_error(path, error))?;
    let metadata = file.metadata().map_err(|error| io_error(path, error))?;
    if !metadata.is_file() || metadata.len() != size {
        return Err(ContractError::InvalidStructure(format!(
            "offline analysis input size changed: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(path, error))?;
    if bytes.len() as u64 != size {
        return Err(ContractError::InvalidStructure(format!(
            "offline analysis input size changed: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

/// Snapshot the actual filesystem input without importing Python modules.
/// File bytes and symlink destinations both participate; timestamps are not
/// integrity evidence. Directory enumeration includes absent import candidates.
pub fn capture_analysis_input(
    path: &Path,
    enumerate_directory: bool,
) -> Result<AnalysisInputState, ContractError> {
    let filters = if enumerate_directory {
        vec![AnalysisDirectoryFilter::All]
    } else {
        Vec::new()
    };
    capture_analysis_input_with_filters(path, &filters)
}

/// Capture only explicitly consumed directory views, while retaining complete
/// byte digests and canonical targets for file inputs. Direct and missing file
/// observations are never filtered.
pub fn capture_analysis_input_with_filters(
    path: &Path,
    filters: &[AnalysisDirectoryFilter],
) -> Result<AnalysisInputState, ContractError> {
    capture_analysis_input_bounded(path, filters, None)
}

fn capture_analysis_input_bounded(
    path: &Path,
    filters: &[AnalysisDirectoryFilter],
    expected_file_size: Option<u64>,
) -> Result<AnalysisInputState, ContractError> {
    absolute(path)?;
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(AnalysisInputState::Missing);
        }
        Err(error) => return Err(io_error(path, error)),
    };
    let canonical_path = path.canonicalize().map_err(|error| io_error(path, error))?;
    if metadata.is_file() {
        if expected_file_size.is_some_and(|size| metadata.len() != size) {
            return Err(ContractError::InvalidStructure(format!(
                "offline analysis input size changed: {}",
                path.display()
            )));
        }
        let bytes = if let Some(size) = expected_file_size {
            // Bound a racing growth/replacement as well as the initial stat;
            // never allocate an attacker's newly enlarged input at startup.
            read_file_bounded(path, size)?
        } else {
            fs::read(path).map_err(|error| io_error(path, error))?
        };
        return Ok(AnalysisInputState::File {
            canonical_path,
            digest: Fingerprint::digest(&bytes),
            size: bytes.len() as u64,
        });
    }
    if !metadata.is_dir() {
        return Err(ContractError::InvalidStructure(format!(
            "unsupported analysis input kind at {}",
            path.display(),
        )));
    }
    let observations = if !filters.is_empty() {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path).map_err(|error| io_error(path, error))? {
            let entry = entry.map_err(|error| io_error(path, error))?;
            let kind = entry.file_type().map_err(|error| io_error(path, error))?;
            let name = entry.file_name().into_string().map_err(|_| {
                ContractError::InvalidStructure("non-UTF-8 analysis directory entry".into())
            })?;
            entries.push((name, kind.is_file(), kind.is_dir(), kind.is_symlink()));
        }
        entries.sort();
        let mut filters = filters.to_vec();
        filters.sort();
        filters.dedup();
        filters
            .into_iter()
            .map(|filter| {
                let selected: Vec<_> = entries
                    .iter()
                    .filter(|(name, file, directory, symlink)| {
                        filter.includes(name, *file, *directory, *symlink)
                    })
                    .collect();
                Ok(AnalysisDirectoryObservation {
                    filter,
                    entries: Fingerprint::digest(crate::artifact::canonical_bytes(&selected)?),
                })
            })
            .collect::<Result<Vec<_>, ContractError>>()?
    } else {
        Vec::new()
    };
    Ok(AnalysisInputState::Directory {
        canonical_path,
        observations,
    })
}

pub fn verify_analysis_inputs(inputs: &[AnalysisInput]) -> Result<(), ContractError> {
    for input in inputs {
        let filters = match &input.state {
            AnalysisInputState::Directory { observations, .. } => observations
                .iter()
                .map(|observation| observation.filter.clone())
                .collect(),
            _ => Vec::new(),
        };
        let expected_size = match input.state {
            AnalysisInputState::File { size, .. } => size,
            _ => 0,
        };
        if capture_analysis_input_bounded(&input.path, &filters, Some(expected_size))?
            != input.state
        {
            return Err(ContractError::InvalidStructure(format!(
                "offline analysis input changed: {}",
                input.path.display(),
            )));
        }
    }
    Ok(())
}
