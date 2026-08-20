//! Native-startup-owned artifact authentication, before lowering or execution.
//!
//! This object contains only owned Rust data and file descriptors. It neither
//! imports a checker/Python module nor retains Python objects in hidden cycles.
//! Authenticated source facts remain proposals: publishing runtime capabilities
//! still requires the actual module, function, and type construction barriers.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use pyo3::ffi;
use pyo3::prelude::*;
use soac_contracts::{
    AnalysisInputState, ArtifactExpectations, ArtifactTrustAnchor, CompleteArtifactGeneration,
    ContractError, Fingerprint, SourceDialect, StrictArtifactDeployment, VerifiedModuleTypeFacts,
    verify_analysis_inputs, verify_complete_generation, verify_manifest,
};

#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(target_os = "linux")]
use std::os::unix::io::{AsRawFd, FromRawFd};

const MAX_STARTUP_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFEST_READ: usize = 64 * 1024 * 1024;
const MAX_SHARD_READ: usize = 16 * 1024 * 1024;

unsafe extern "C" {
    fn PySoac_GetStrictConfig(
        bytes: *mut *const c_char,
        length: *mut ffi::Py_ssize_t,
        path: *mut *const libc::wchar_t,
    ) -> libc::c_int;
    fn PySoac_GetStrictRuntimeUnavailableError() -> *mut ffi::PyObject;
    fn PySoac_GetInterpreterPrefix() -> *const libc::wchar_t;
}

struct StartupSnapshot {
    bytes: Arc<[u8]>,
    path: PathBuf,
}

impl StartupSnapshot {
    fn capture(py: Python<'_>) -> PyResult<Option<Self>> {
        let mut bytes = std::ptr::null();
        let mut length = 0;
        let mut path = std::ptr::null();
        let present = unsafe { PySoac_GetStrictConfig(&mut bytes, &mut length, &mut path) };
        if present < 0 {
            return Err(PyErr::fetch(py));
        }
        if present == 0 {
            return Ok(None);
        }
        if present != 1
            || bytes.is_null()
            || path.is_null()
            || length <= 0
            || length as usize > MAX_STARTUP_BYTES
        {
            return Err(strict_runtime_unavailable(
                py,
                "invalid native strict startup snapshot",
            ));
        }
        // Native storage is immutable until interpreter deletion. Copy it while
        // attached, and keep no borrowed Python or native pointers afterwards.
        let bytes: Arc<[u8]> =
            unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), length as usize) }.into();
        let path_object = unsafe { ffi::PyUnicode_FromWideChar(path, -1) };
        if path_object.is_null() {
            return Err(PyErr::fetch(py));
        }
        let path: String = unsafe { Bound::<PyAny>::from_owned_ptr(py, path_object) }.extract()?;
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(strict_runtime_unavailable(
                py,
                "strict startup descriptor path must be absolute",
            ));
        }
        Ok(Some(Self { bytes, path }))
    }
}

/// Use the shared interpreter-owned exception, never a Rust/Python global cache.
pub fn strict_runtime_unavailable(py: Python<'_>, message: impl AsRef<str>) -> PyErr {
    let exception = unsafe { PySoac_GetStrictRuntimeUnavailableError() };
    if exception.is_null() {
        return PyErr::fetch(py);
    }
    let message = CString::new(message.as_ref().replace('\0', "\\0"))
        .expect("exception text has no embedded NUL");
    unsafe { ffi::PyErr_SetString(exception, message.as_ptr()) };
    PyErr::fetch(py)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeFileIdentity {
    device_major: u64,
    device_minor: u64,
    inode: u64,
}

impl NativeFileIdentity {
    #[cfg(target_os = "linux")]
    fn from_path(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("read native interpreter identity at {}", path.display()))?;
        ensure!(
            metadata.is_file(),
            "interpreter identity is not a regular file: {}",
            path.display()
        );
        Ok(Self {
            device_major: libc::major(metadata.dev()) as u64,
            device_minor: libc::minor(metadata.dev()) as u64,
            inode: metadata.ino(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn from_path(_path: &Path) -> Result<Self> {
        bail!("strict native interpreter identity currently requires Linux");
    }
}

#[derive(Clone)]
struct NativeInterpreter {
    interpreter_id: i64,
    version: [u8; 2],
    platform: String,
    prefix: PathBuf,
    executable: NativeFileIdentity,
    library: NativeFileIdentity,
}

impl NativeInterpreter {
    #[cfg(target_os = "linux")]
    fn observe(py: Python<'_>) -> Result<Self> {
        let interpreter_id =
            unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        ensure!(interpreter_id >= 0, "cannot identify current interpreter");
        let version = unsafe { CStr::from_ptr(ffi::Py_GetVersion()) }.to_str()?;
        let mut components = version.split('.');
        let major = components
            .next()
            .context("missing native Python major version")?
            .parse()?;
        let minor = components
            .next()
            .context("missing native Python minor version")?
            .parse()?;
        let platform = unsafe { CStr::from_ptr(ffi::Py_GetPlatform()) }
            .to_str()?
            .to_owned();
        // This is the current interpreter's immutable native configuration,
        // not sys.prefix, PyConfig_Get's mutable attribute view, or the
        // process-global path configuration used by the stable Py_GetPrefix.
        let prefix = unsafe { PySoac_GetInterpreterPrefix() };
        ensure!(
            !prefix.is_null(),
            "native interpreter prefix is unavailable"
        );
        let prefix: String = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(py, ffi::PyUnicode_FromWideChar(prefix, -1))?
        }
        .extract()?;
        let prefix = PathBuf::from(prefix);
        ensure!(
            prefix.is_absolute(),
            "native interpreter prefix is not absolute"
        );
        let address = ffi::Py_GetVersion as *const c_void as usize;
        let maps =
            fs::read_to_string("/proc/self/maps").context("read actual native library mappings")?;
        let library = mapped_file_identity(&maps, address)?;
        Ok(Self {
            interpreter_id,
            version: [major, minor],
            platform,
            prefix,
            executable: NativeFileIdentity::from_path(Path::new("/proc/self/exe"))?,
            library,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn observe(_py: Python<'_>) -> Result<Self> {
        bail!("strict native interpreter identity currently requires Linux");
    }
}

fn mapped_file_identity(maps: &str, address: usize) -> Result<NativeFileIdentity> {
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let range = fields.next().context("invalid native mapping address")?;
        let (start, end) = range
            .split_once('-')
            .context("invalid native mapping range")?;
        let start = usize::from_str_radix(start, 16)?;
        let end = usize::from_str_radix(end, 16)?;
        if !(start..end).contains(&address) {
            continue;
        }
        let permissions = fields
            .next()
            .context("missing native mapping permissions")?;
        ensure!(
            permissions.contains('x'),
            "CPython symbol is not in an executable mapping"
        );
        let _offset = fields.next().context("missing native mapping offset")?;
        let device = fields.next().context("missing native mapping device")?;
        let inode = fields
            .next()
            .context("missing native mapping inode")?
            .parse()?;
        let (major, minor) = device
            .split_once(':')
            .context("invalid native mapping device")?;
        ensure!(
            inode != 0 && !line.ends_with(" (deleted)"),
            "CPython library mapping has no live file identity"
        );
        return Ok(NativeFileIdentity {
            device_major: u64::from_str_radix(major, 16)?,
            device_minor: u64::from_str_radix(minor, 16)?,
            inode,
        });
    }
    bail!("cannot locate the mapping containing the actual Py_GetVersion symbol");
}

fn verify_native_interpreter(
    deployment: &StrictArtifactDeployment,
    actual: &NativeInterpreter,
) -> Result<()> {
    let expected = &deployment.target_interpreter;
    ensure!(
        actual.version == expected.version,
        "actual interpreter version differs from startup target"
    );
    ensure!(
        actual.platform == expected.platform,
        "actual interpreter platform differs from startup target"
    );
    ensure!(
        actual.prefix == Path::new(&expected.prefix),
        "actual interpreter prefix differs from startup target"
    );
    ensure!(
        expected
            .configuration
            .get("Py_ENABLE_SHARED")
            .and_then(serde_json::Value::as_u64)
            == Some(1),
        "strict runtime requires the selected shared-library CPython build"
    );
    ensure!(
        NativeFileIdentity::from_path(Path::new(&expected.executable))? == actual.executable,
        "running executable is not the startup-selected interpreter"
    );
    let files = expected
        .abi_files
        .iter()
        .map(|path| NativeFileIdentity::from_path(Path::new(path)))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        files.len() == 2
            && actual.executable != actual.library
            && files.contains(&actual.executable)
            && files.contains(&actual.library),
        "actual mapped libpython does not match startup-selected ABI files"
    );
    ensure!(
        expected.abi_fingerprint(&deployment.analysis_inputs)?
            == deployment.environment.cpython_abi_fingerprint,
        "startup interpreter ABI fingerprint is inconsistent"
    );
    Ok(())
}

fn verify_observed_environment(deployment: &StrictArtifactDeployment) -> Result<()> {
    verify_analysis_inputs(&deployment.analysis_inputs)?;
    verify_observed_environment_variables(deployment)
}

fn verify_observed_environment_variables(deployment: &StrictArtifactDeployment) -> Result<()> {
    // Environment values are observations checked against immutable startup
    // expectations, never a source of trust keys, paths, or replacement policy.
    for variable in &deployment.analysis_environment {
        let current = match std::env::var(&variable.name) {
            Ok(value) => Some(Fingerprint::digest(value)),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                bail!("analysis environment is not UTF-8: {}", variable.name)
            }
        };
        ensure!(
            current == variable.value,
            "offline analysis environment changed: {}",
            variable.name
        );
    }
    Ok(())
}

/// Stable open directory handles prevent later path replacement from switching
/// the selected generation. File contents are still authenticated on every read.
struct ArtifactDirectory {
    generation: File,
    modules: File,
}

impl ArtifactDirectory {
    #[cfg(target_os = "linux")]
    fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "artifact generation path must be absolute"
        );
        let generation = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .with_context(|| format!("open artifact generation {}", path.display()))?;
        let modules = open_relative(&generation, "modules", libc::O_DIRECTORY)?;
        Ok(Self {
            generation,
            modules,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn open(_path: &Path) -> Result<Self> {
        bail!("strict artifact loading currently requires Linux");
    }

    fn manifest(&self) -> Result<Vec<u8>> {
        read_relative(&self.generation, "manifest.json", MAX_MANIFEST_READ)
    }

    fn shard(&self, digest: Fingerprint) -> Result<Vec<u8>> {
        read_relative(
            &self.modules,
            &format!("{digest}.soac-types"),
            MAX_SHARD_READ,
        )
    }
}

#[cfg(target_os = "linux")]
fn open_relative(directory: &File, name: &str, flags: i32) -> Result<File> {
    ensure!(
        !name.contains(['/', '\\', '\0']) && name != "." && name != "..",
        "invalid artifact filename"
    );
    let name = CString::new(name)?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | flags,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error()).context("open immutable artifact file");
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(not(target_os = "linux"))]
fn open_relative(_directory: &File, _name: &str, _flags: i32) -> Result<File> {
    bail!("strict artifact loading currently requires Linux");
}

fn read_relative(directory: &File, name: &str, limit: usize) -> Result<Vec<u8>> {
    let file = open_relative(directory, name, 0)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "artifact {name} is not a regular file");
    ensure!(
        metadata.len() <= limit as u64,
        "artifact {name} exceeds {limit} byte read limit"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "artifact {name} exceeds {limit} byte read limit"
    );
    Ok(bytes)
}

/// An explicit, interpreter-bound deployment loader. This is not a global
/// cache; a compile session owns it and must not reuse it in another interpreter.
pub struct StrictArtifactLoader {
    startup_path: PathBuf,
    startup_identity: Fingerprint,
    interpreter_id: i64,
    deployment: StrictArtifactDeployment,
    artifacts: ArtifactDirectory,
    generation: CompleteArtifactGeneration,
    selected_names: BTreeMap<String, usize>,
    selected_paths: BTreeMap<PathBuf, usize>,
}

impl StrictArtifactLoader {
    /// Capture only interpreter-owned startup bytes. An absent opt-in returns
    /// None; malformed or stale opted-in deployments fail without a fallback.
    pub fn capture(py: Python<'_>) -> PyResult<Option<Self>> {
        let Some(snapshot) = StartupSnapshot::capture(py)? else {
            return Ok(None);
        };
        let path = snapshot.path.clone();
        let result = NativeInterpreter::observe(py)
            .and_then(|actual| Self::from_snapshot(snapshot, &actual));
        result.map(Some).map_err(|error| {
            strict_runtime_unavailable(
                py,
                format!("strict startup deployment {}: {error:#}", path.display()),
            )
        })
    }

    fn from_snapshot(snapshot: StartupSnapshot, actual: &NativeInterpreter) -> Result<Self> {
        ensure!(
            !snapshot.bytes.is_empty() && snapshot.bytes.len() <= MAX_STARTUP_BYTES,
            "strict startup descriptor exceeds its bounded framing"
        );
        ensure!(
            snapshot.path.is_absolute(),
            "strict startup descriptor path must be absolute"
        );
        let deployment: StrictArtifactDeployment =
            serde_json::from_slice(&snapshot.bytes).context("parse captured startup deployment")?;
        // No Python callbacks or publication occur during this constructor.
        // Reuse only its independently observed inputs, never observations
        // from an earlier constructor or a later module admission.
        let analysis = deployment.verified_analysis_snapshot()?;
        verify_observed_environment_variables(&deployment)?;
        verify_native_interpreter(&deployment, actual)?;
        let artifacts = ArtifactDirectory::open(&deployment.artifact_directory)?;
        let manifest = verify_manifest(
            &artifacts.manifest()?,
            &ArtifactTrustAnchor::from_bytes(&deployment.trust_anchor)?,
            &ArtifactExpectations {
                generation: deployment.generation,
                environment: deployment.environment.clone(),
            },
        )?;
        let generation = verify_complete_generation(manifest, |digest| {
            artifacts.shard(digest).map_err(|error| {
                ContractError::InvalidStructure(format!(
                    "read complete generation shard {digest}: {error:#}"
                ))
            })
        })?;
        ensure!(
            generation.manifest().manifest().modules.len() == deployment.modules.len(),
            "signed module catalog differs from startup selection"
        );
        let mut selected_names = BTreeMap::new();
        let mut selected_paths = BTreeMap::new();
        for (position, module) in deployment.modules.iter().enumerate() {
            let index = generation.manifest().module_index(&module.module_name)?;
            ensure!(
                index.effective_policy == module.policy.fingerprint()?,
                "startup policy differs from signed module {}",
                module.module_name
            );
            ensure!(
                index.consumed_dependencies.as_slice()
                    == analysis.dependencies(&module.module_name)?,
                "current dependencies differ from signed module {}",
                module.module_name
            );
            let path = module.source_path.canonicalize().with_context(|| {
                format!("resolve selected source {}", module.source_path.display())
            })?;
            ensure!(
                selected_names
                    .insert(module.module_name.clone(), position)
                    .is_none()
                    && selected_paths.insert(path, position).is_none(),
                "startup module selection is ambiguous"
            );
        }
        let startup_identity = Fingerprint::digest(serde_json::to_vec(&(
            snapshot
                .path
                .to_str()
                .context("startup path is not UTF-8")?,
            Fingerprint::digest(&snapshot.bytes),
        ))?);
        drop(analysis);
        Ok(Self {
            startup_path: snapshot.path,
            startup_identity,
            interpreter_id: actual.interpreter_id,
            deployment,
            artifacts,
            generation,
            selected_names,
            selected_paths,
        })
    }

    pub fn interpreter_id(&self) -> i64 {
        self.interpreter_id
    }

    /// Select only a startup-declared name/path pair, without reading source
    /// bytes or granting execution authority. Ordinary loaders may keep their
    /// native coding-cookie, bytecode-cache, or custom source behavior. The
    /// subsequent load_module call independently repeats these identity checks
    /// and authenticates the actual source and environment.
    pub fn selects_source(
        &self,
        py: Python<'_>,
        module_name: &str,
        source_path: &Path,
    ) -> PyResult<bool> {
        let interpreter_id =
            unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter_id < 0 {
            return Err(PyErr::fetch(py));
        }
        self.selected_source(module_name, source_path, interpreter_id)
            .map(|selected| selected.is_some())
            .map_err(|error| {
                strict_runtime_unavailable(
                    py,
                    format!(
                        "strict source selection {module_name} from {}: {error:#}",
                        source_path.display()
                    ),
                )
            })
    }

    /// Authenticate the exact source supplied to lowering. Ordinary unselected
    /// modules return None; no selected identity/path/source mismatch does.
    pub fn load_module(
        &self,
        py: Python<'_>,
        module_name: &str,
        source_path: &Path,
        source: &[u8],
    ) -> PyResult<Option<Arc<VerifiedStrictModule>>> {
        NativeInterpreter::observe(py)
            .and_then(|actual| self.load_verified(module_name, source_path, source, &actual))
            .map_err(|error| {
                strict_runtime_unavailable(
                    py,
                    format!(
                        "strict module {module_name} from {} (startup {}): {error:#}",
                        source_path.display(),
                        self.startup_path.display()
                    ),
                )
            })
    }

    fn selected_source(
        &self,
        module_name: &str,
        source_path: &Path,
        interpreter_id: i64,
    ) -> Result<Option<(usize, PathBuf)>> {
        ensure!(
            interpreter_id == self.interpreter_id,
            "strict deployment belongs to another interpreter"
        );
        let Some(&position) = self.selected_names.get(module_name) else {
            if let Ok(path) = source_path.canonicalize() {
                ensure!(
                    !self.selected_paths.contains_key(&path),
                    "selected strict source was requested under a different module identity"
                );
            }
            return Ok(None);
        };
        let path = source_path
            .canonicalize()
            .with_context(|| format!("resolve actual selected source {}", source_path.display()))?;
        ensure!(
            self.selected_paths.get(&path) == Some(&position),
            "actual source path differs from startup selection"
        );
        Ok(Some((position, path)))
    }

    fn load_verified(
        &self,
        module_name: &str,
        source_path: &Path,
        source: &[u8],
        actual: &NativeInterpreter,
    ) -> Result<Option<Arc<VerifiedStrictModule>>> {
        let Some((position, path)) =
            self.selected_source(module_name, source_path, actual.interpreter_id)?
        else {
            return Ok(None);
        };
        let selected = &self.deployment.modules[position];
        verify_native_interpreter(&self.deployment, actual)?;
        verify_observed_environment(&self.deployment)?;
        let source_input = self
            .deployment
            .analysis_inputs
            .iter()
            .find(|input| input.path == selected.source_path)
            .context("selected source is missing from startup analysis observations")?;
        ensure!(
            matches!(&source_input.state,
            AnalysisInputState::File { canonical_path, digest, size }
            if canonical_path == &path && *digest == Fingerprint::digest(source)
                && *size == source.len() as u64),
            "source bytes differ from the startup-observed selected file"
        );
        let index = self.generation.manifest().module_index(module_name)?;
        let dependencies = self
            .deployment
            .verified_analysis_dependencies(module_name)?;
        let shard = self.artifacts.shard(index.shard_digest)?;
        let type_facts = self.generation.manifest().verify_module(
            module_name,
            source,
            &selected.policy,
            &dependencies,
            &shard,
        )?;
        ensure!(
            type_facts.facts().source_dialect == SourceDialect::SoacStrict,
            "selected source lacks an authenticated strict dialect"
        );
        Ok(Some(Arc::new(VerifiedStrictModule {
            interpreter_id: self.interpreter_id,
            startup_identity: self.startup_identity,
            source_path: path,
            source: source.into(),
            type_facts,
        })))
    }
}

/// Authenticated bytes and proposals, not a sealed module or optimizer permit.
/// No public constructor or deserializer can manufacture this value.
pub struct VerifiedStrictModule {
    interpreter_id: i64,
    startup_identity: Fingerprint,
    source_path: PathBuf,
    source: Arc<[u8]>,
    type_facts: VerifiedModuleTypeFacts,
}

impl VerifiedStrictModule {
    pub fn interpreter_id(&self) -> i64 {
        self.interpreter_id
    }
    pub fn startup_identity(&self) -> Fingerprint {
        self.startup_identity
    }
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
    pub fn source(&self) -> &[u8] {
        &self.source
    }
    pub fn type_facts(&self) -> &VerifiedModuleTypeFacts {
        &self.type_facts
    }

    /// Runtime-kernel fixture only: the facts must already pass the real
    /// signature/shard verifier. Production startup/deployment authentication
    /// is tested through StrictArtifactLoader and subprocess entrypoints, not
    /// inferred from this constructor. It is absent from non-test builds.
    #[cfg(test)]
    pub(crate) fn from_verified_test_facts(
        py: Python<'_>,
        source_path: PathBuf,
        source: Arc<[u8]>,
        type_facts: VerifiedModuleTypeFacts,
    ) -> PyResult<Self> {
        let construct = || -> Result<Self> {
            ensure!(
                source_path.is_absolute(),
                "fixture source path must be absolute"
            );
            let canonical = fs::canonicalize(&source_path)?;
            ensure!(
                canonical == source_path,
                "fixture source path must be canonical"
            );
            let mut observed = Vec::with_capacity(source.len());
            File::open(&source_path)?
                .take(source.len() as u64 + 1)
                .read_to_end(&mut observed)?;
            ensure!(
                observed.as_slice() == source.as_ref(),
                "fixture source file differs from supplied bytes"
            );
            let facts = type_facts.facts();
            ensure!(
                facts.source_dialect == SourceDialect::SoacStrict,
                "fixture requires verified strict facts"
            );
            ensure!(
                facts.source_digest == Fingerprint::digest(&source)
                    && facts.source_size as usize == source.len()
                    && facts.module.source_hash == soac_contracts::legacy_source_hash(&source),
                "fixture source differs from verified signed facts"
            );
            let interpreter_id =
                unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
            ensure!(interpreter_id >= 0, "cannot identify fixture interpreter");
            Ok(Self {
                interpreter_id,
                startup_identity: Fingerprint::digest(b"SOAC-runtime-kernel-test-fixture-v1"),
                source_path,
                source,
                type_facts,
            })
        };
        construct().map_err(|error| {
            strict_runtime_unavailable(py, format!("invalid verified runtime fixture: {error:#}"))
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use soac_contracts::{
        AnalysisDependency, AnalysisDependencySource, AnalysisEnvironmentVariable,
        AnalysisFileConfiguration, AnalysisInput, ArtifactEnvironment, ArtifactSigningKey,
        ConservativeAnalysis, DEPLOYMENT_SCHEMA_VERSION, DeployedModule, InterpreterIdentity,
        ModuleArtifactIndex, ModuleContentId, ModuleTypeFacts, PythonVersion, ResolvedStrictPolicy,
        TypeArtifactManifest, TypingFinalPolicy, capture_analysis_input, encode_module_shard,
        legacy_source_hash, sign_manifest,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SOURCE: &[u8] = b"from __future__ import strict\nvalue = 1\n";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "soac-strict-loader-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create loader fixture: {error}"),
                }
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        directory: TestDirectory,
        deployment: StrictArtifactDeployment,
        actual: NativeInterpreter,
        shards: BTreeMap<String, PathBuf>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_source(SOURCE)
        }

        fn with_source(source: &[u8]) -> Self {
            let directory = TestDirectory::new();
            let root = &directory.0;
            let artifact_directory = root.join("generation");
            fs::create_dir_all(artifact_directory.join("modules")).unwrap();
            fs::write(root.join("python"), b"native executable fixture").unwrap();
            fs::write(root.join("libpython.so"), b"native library fixture").unwrap();
            fs::write(root.join("dependency.pyi"), b"count: int\n").unwrap();
            let modules = ["selected", "other"]
                .into_iter()
                .map(|name| {
                    let path = root.join(format!("{name}.py"));
                    fs::write(&path, source).unwrap();
                    DeployedModule {
                        module_name: name.into(),
                        source_path: path,
                        policy: ResolvedStrictPolicy::default(),
                    }
                })
                .collect::<Vec<_>>();
            let mut inputs = [
                "python",
                "libpython.so",
                "dependency.pyi",
                "selected.py",
                "other.py",
            ]
            .into_iter()
            .map(|name| {
                let path = root.join(name);
                AnalysisInput {
                    state: capture_analysis_input(&path, false).unwrap(),
                    path,
                }
            })
            .collect::<Vec<_>>();
            inputs.sort_by(|left, right| left.path.cmp(&right.path));
            let mut abi_files = vec![
                root.join("python").to_str().unwrap().to_owned(),
                root.join("libpython.so").to_str().unwrap().to_owned(),
            ];
            abi_files.sort();
            let root_string = root.to_str().unwrap().to_owned();
            let target = InterpreterIdentity {
                version: [3, 15],
                platform: "linux".into(),
                prefix: root_string.clone(),
                executable: root.join("python").to_str().unwrap().into(),
                build_directory: root_string.clone(),
                site_packages: Vec::new(),
                real_stdlib: root_string.clone(),
                abi_files,
                configuration_files: Vec::new(),
                configuration: BTreeMap::from([("Py_ENABLE_SHARED".into(), serde_json::json!(1))]),
            };
            let fingerprint = Fingerprint::digest(b"owned fixture environment");
            let environment = ArtifactEnvironment {
                ty_revision: "fixture-checker".into(),
                checker_source_fingerprint: fingerprint,
                exporter_revision: "fixture-exporter".into(),
                python_version: PythonVersion {
                    major: 3,
                    minor: 15,
                },
                python_platform: "linux".into(),
                cpython_abi_fingerprint: target.abi_fingerprint(&inputs).unwrap(),
                normalized_project_policy: fingerprint,
                resolved_typechecker_configuration: fingerprint,
                import_search_path: fingerprint,
                typeshed_fingerprint: fingerprint,
                installed_stub_fingerprint: fingerprint,
                installed_dependency_fingerprint: fingerprint,
                analysis: ConservativeAnalysis::default(),
            };
            let dependency_source = fs::read(root.join("dependency.pyi")).unwrap();
            let dependency = AnalysisDependency {
                importer_module: "selected".into(),
                module: ModuleContentId::new("dependency", legacy_source_hash(&dependency_source)),
                source: AnalysisDependencySource::System {
                    path: root.join("dependency.pyi"),
                },
                source_digest: Fingerprint::digest(&dependency_source),
                source_size: dependency_source.len() as u32,
                configuration: AnalysisFileConfiguration {
                    python_version: environment.python_version,
                    python_platform: "linux".into(),
                    analysis: ConservativeAnalysis::default(),
                    respect_type_ignore_comments: true,
                    import_search_paths: vec![root_string],
                    enabled_diagnostics: BTreeMap::new(),
                },
            };
            let mut indices = Vec::new();
            let mut shards = BTreeMap::new();
            for module in &modules {
                let mut facts = ModuleTypeFacts::new(
                    &module.module_name,
                    source,
                    SourceDialect::SoacStrict,
                    module.policy.clone(),
                )
                .unwrap();
                if module.module_name == "selected" {
                    facts.consumed_dependencies.push(
                        dependency
                            .fingerprint(&environment, &modules, &inputs)
                            .unwrap(),
                    );
                }
                let shard = encode_module_shard(&facts).unwrap();
                let path = artifact_directory.join("modules").join(shard.file_name());
                fs::write(&path, shard.bytes()).unwrap();
                shards.insert(module.module_name.clone(), path);
                indices.push(ModuleArtifactIndex::from_shard(&shard).unwrap());
            }
            let manifest = TypeArtifactManifest::new(environment.clone(), indices).unwrap();
            let key = ArtifactSigningKey::from_bytes(&[41; 32]);
            fs::write(
                artifact_directory.join("manifest.json"),
                sign_manifest(&manifest, &key).unwrap(),
            )
            .unwrap();
            let actual = NativeInterpreter {
                interpreter_id: 31,
                version: [3, 15],
                platform: "linux".into(),
                prefix: root.clone(),
                executable: NativeFileIdentity::from_path(&root.join("python")).unwrap(),
                library: NativeFileIdentity::from_path(&root.join("libpython.so")).unwrap(),
            };
            let deployment = StrictArtifactDeployment {
                schema_version: DEPLOYMENT_SCHEMA_VERSION,
                artifact_directory,
                generation: manifest.generation,
                environment,
                target_interpreter: target,
                trust_anchor: key.trust_anchor().to_bytes(),
                modules,
                analysis_dependencies: vec![dependency],
                analysis_inputs: inputs,
                analysis_environment: Vec::new(),
            };
            Self {
                directory,
                deployment,
                actual,
                shards,
            }
        }

        fn snapshot(&self) -> StartupSnapshot {
            StartupSnapshot {
                path: self.directory.0.join("startup.json"),
                bytes: serde_json::to_vec(&self.deployment).unwrap().into(),
            }
        }

        fn loader(&self) -> Result<StrictArtifactLoader> {
            StrictArtifactLoader::from_snapshot(self.snapshot(), &self.actual)
        }

        fn load(&self, loader: &StrictArtifactLoader) -> Result<Option<Arc<VerifiedStrictModule>>> {
            loader.load_verified(
                "selected",
                &self.directory.0.join("selected.py"),
                SOURCE,
                &self.actual,
            )
        }
    }

    #[test]
    fn native_annotation_provider_entry_projects_public_format_to_body_storage() {
        let source = b"from __future__ import strict\nvalue: int = 1\n";
        let fixture = Fixture::with_source(source);
        let loader = fixture.loader().unwrap();
        let verified = loader
            .load_verified(
                "selected",
                &fixture.directory.0.join("selected.py"),
                source,
                &fixture.actual,
            )
            .unwrap()
            .unwrap();
        let module = soac_lowering::lower_source_to_blockpy_module_with_tracker(
            std::str::from_utf8(source).unwrap(),
            soac_core::block_py::ModuleNameGen::new(32),
            &mut soac_core::pass_tracker::NoopPassTracker,
            soac_lowering::LoweringOptions {
                strict_facts: Some(Arc::new(verified.type_facts().clone())),
                ..Default::default()
            },
        )
        .unwrap();
        let provider = module
            .callable_defs
            .iter()
            .find(|function| function.scope.annotation_provider.is_some())
            .unwrap();
        assert_eq!(provider.params.params[0].name, "format");
        let body_name = &provider.body_params().params[0].name;
        assert_ne!(body_name, "format");
        assert!(
            provider
                .public_storage_layout()
                .unwrap()
                .stack_slots()
                .contains(body_name)
        );
        crate::jit::RuntimeFunctionEntryPlan::from_function(provider).unwrap();
    }

    #[test]
    fn complete_generation_authenticates_exact_source_and_keeps_proposals_owned() {
        let fixture = Fixture::new();
        let loader = fixture.loader().unwrap();
        let module = fixture.load(&loader).unwrap().unwrap();
        assert_eq!(module.source(), SOURCE);
        assert_eq!(module.type_facts().facts().module.module_name, "selected");
        assert_eq!(
            module.type_facts().generation(),
            fixture.deployment.generation
        );
        assert_eq!(module.interpreter_id(), fixture.actual.interpreter_id);
        assert!(
            loader
                .load_verified("ordinary", Path::new("<ordinary>"), b"", &fixture.actual)
                .unwrap()
                .is_none()
        );
        assert!(
            loader
                .load_verified(
                    "alias",
                    &fixture.directory.0.join("selected.py"),
                    SOURCE,
                    &fixture.actual
                )
                .is_err()
        );
    }

    #[test]
    fn startup_authority_is_the_captured_bytes_not_a_later_path_read() {
        let fixture = Fixture::new();
        let snapshot = fixture.snapshot();
        fs::write(&snapshot.path, b"not the captured authority").unwrap();
        let loader = StrictArtifactLoader::from_snapshot(snapshot, &fixture.actual).unwrap();
        assert!(fixture.load(&loader).unwrap().is_some());
    }

    #[test]
    fn missing_unrequested_shard_rejects_the_entire_generation() {
        let fixture = Fixture::new();
        fs::remove_file(&fixture.shards["other"]).unwrap();
        assert!(fixture.loader().is_err());
    }

    #[test]
    fn startup_policy_and_module_selection_must_match_the_signed_catalog() {
        let mut fixture = Fixture::new();
        fixture.deployment.modules[0].policy.typing_final_policy = TypingFinalPolicy::Advisory;
        assert!(fixture.loader().is_err());
        fixture.deployment.modules[0].policy = ResolvedStrictPolicy::default();
        fixture.deployment.modules.pop();
        assert!(fixture.loader().is_err());
    }

    #[test]
    fn observed_environment_is_checked_without_becoming_replacement_authority() {
        let mut fixture = Fixture::new();
        let name = format!(
            "SOAC_LOADER_FIXTURE_{}",
            fixture.directory.0.file_name().unwrap().to_str().unwrap()
        );
        assert!(std::env::var_os(&name).is_none());
        fixture
            .deployment
            .analysis_environment
            .push(AnalysisEnvironmentVariable {
                name,
                value: Some(Fingerprint::digest(b"missing expected environment value")),
            });
        assert!(fixture.loader().is_err());
        fixture.deployment.analysis_environment[0].value = None;
        assert!(fixture.loader().is_ok());
    }

    #[test]
    fn wrong_startup_key_and_replaced_shards_cannot_supply_facts() {
        let mut fixture = Fixture::new();
        let loader = fixture.loader().unwrap();
        fixture.deployment.trust_anchor = ArtifactSigningKey::from_bytes(&[42; 32])
            .trust_anchor()
            .to_bytes();
        let error = fixture.loader().err().unwrap();
        assert!(matches!(
            error.downcast_ref::<ContractError>(),
            Some(ContractError::UntrustedSignature)
        ));
        fs::write(&fixture.shards["selected"], b"{}").unwrap();
        assert!(fixture.load(&loader).is_err());
    }

    #[test]
    fn source_selection_is_identity_only_and_cannot_authenticate_changed_bytes() {
        let fixture = Fixture::new();
        let loader = fixture.loader().unwrap();
        let source = fixture.directory.0.join("selected.py");
        let ordinary = fixture.directory.0.join("not-a-source-file");
        let interpreter = fixture.actual.interpreter_id;
        assert!(
            loader
                .selected_source("ordinary", &ordinary, interpreter)
                .unwrap()
                .is_none()
        );
        assert!(
            loader
                .selected_source("selected", &source, interpreter)
                .unwrap()
                .is_some()
        );
        assert!(
            loader
                .selected_source("alias", &source, interpreter)
                .is_err()
        );
        assert!(
            loader
                .selected_source("selected", &ordinary, interpreter)
                .is_err()
        );
        assert!(
            loader
                .selected_source("ordinary", &ordinary, interpreter + 1)
                .is_err()
        );
        fs::write(&source, b"changed selected source").unwrap();
        assert!(
            loader
                .selected_source("selected", &source, interpreter)
                .unwrap()
                .is_some()
        );
        assert!(
            loader
                .load_verified(
                    "selected",
                    &source,
                    b"changed selected source",
                    &fixture.actual
                )
                .is_err()
        );
    }

    #[test]
    fn supplied_source_path_bytes_and_consumed_dependency_are_independently_checked() {
        let fixture = Fixture::new();
        let loader = fixture.loader().unwrap();
        assert!(fixture.load(&loader).unwrap().is_some());
        assert!(
            loader
                .load_verified(
                    "selected",
                    &fixture.directory.0.join("other.py"),
                    SOURCE,
                    &fixture.actual
                )
                .is_err()
        );
        assert!(
            loader
                .load_verified(
                    "selected",
                    &fixture.directory.0.join("selected.py"),
                    b"changed",
                    &fixture.actual
                )
                .is_err()
        );
        fs::write(fixture.directory.0.join("dependency.pyi"), b"other: int\n").unwrap();
        assert!(fixture.load(&loader).is_err());
        fs::write(fixture.directory.0.join("dependency.pyi"), b"count: int\n").unwrap();
        assert!(fixture.load(&loader).unwrap().is_some());
    }

    #[test]
    fn native_interpreter_identity_and_file_bytes_are_not_manifest_claims() {
        let fixture = Fixture::new();
        let loader = fixture.loader().unwrap();
        let mut different = fixture.actual.clone();
        different.prefix = different.prefix.join("another-venv");
        assert!(StrictArtifactLoader::from_snapshot(fixture.snapshot(), &different).is_err());
        assert!(
            loader
                .load_verified(
                    "selected",
                    &fixture.directory.0.join("selected.py"),
                    SOURCE,
                    &different
                )
                .is_err()
        );
        different = fixture.actual.clone();
        different.library.inode += 1;
        assert!(StrictArtifactLoader::from_snapshot(fixture.snapshot(), &different).is_err());
        different = fixture.actual.clone();
        different.version[1] = 14;
        assert!(StrictArtifactLoader::from_snapshot(fixture.snapshot(), &different).is_err());
        different = fixture.actual.clone();
        different.interpreter_id += 1;
        assert!(
            loader
                .load_verified(
                    "selected",
                    &fixture.directory.0.join("selected.py"),
                    SOURCE,
                    &different
                )
                .is_err()
        );
        fs::write(
            fixture.directory.0.join("libpython.so"),
            b"changed library fixture",
        )
        .unwrap();
        assert!(fixture.load(&loader).is_err());
    }

    #[test]
    fn native_mapping_selection_uses_symbol_address_and_kernel_file_identity() {
        let maps = "1000-2000 r-xp 0000 08:02 41 /some/library.so\n2000-3000 r-xp 0000 08:02 42 /other.so\n";
        assert_eq!(
            mapped_file_identity(maps, 0x2200).unwrap(),
            NativeFileIdentity {
                device_major: 8,
                device_minor: 2,
                inode: 42,
            }
        );
        assert!(mapped_file_identity(maps, 0x3000).is_err());
        assert!(
            mapped_file_identity("1000-2000 r-xp 0000 08:02 41 /gone (deleted)\n", 0x1200).is_err()
        );
    }

    #[test]
    fn artifact_reads_reject_symlinks_and_oversized_files_before_loading_bytes() {
        let fixture = Fixture::new();
        let directory = ArtifactDirectory::open(&fixture.deployment.artifact_directory).unwrap();
        assert!(read_relative(&directory.generation, "manifest.json", 1).is_err());
        let path = fixture.deployment.artifact_directory.join("manifest.json");
        let backup = fixture.directory.0.join("manifest-backup");
        fs::rename(&path, &backup).unwrap();
        std::os::unix::fs::symlink(&backup, &path).unwrap();
        assert!(directory.manifest().is_err());
        assert!(fixture.loader().is_err());
    }
}
