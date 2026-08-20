use libc::wchar_t;
use pyo3::ffi;
use pyo3::prelude::*;
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct EmbeddedPythonConfig {
    program_name: String,
    python_home: PathBuf,
    search_paths: Vec<PathBuf>,
}

impl EmbeddedPythonConfig {
    pub fn vendored(repo_root: impl AsRef<Path>, program_name: impl Into<String>) -> Self {
        let repo_root = repo_root.as_ref();
        // Preserve the build selected when compiling standalone binaries, while
        // allowing an explicit runtime environment to select another build.
        let build_dir = std::env::var_os("CPYTHON_LIB_DIR")
            .or_else(|| std::env::var_os("CPYTHON_BUILD_DIR"))
            .or_else(|| option_env!("CPYTHON_LIB_DIR").map(Into::into))
            .or_else(|| option_env!("CPYTHON_BUILD_DIR").map(Into::into))
            .map(|path| repo_root.join(path))
            .unwrap_or_else(|| vendored_python_home(repo_root));
        Self::vendored_with_build_dir(
            &vendored_python_home(repo_root),
            program_name.into(),
            &build_dir,
        )
    }

    fn vendored_with_build_dir(source_dir: &Path, program_name: String, build_dir: &Path) -> Self {
        let python_home = source_dir.to_path_buf();
        let mut search_paths = vec![python_home.join("Lib")];
        if let Some(build_lib_dir) = vendored_python_build_lib_dir(build_dir) {
            search_paths.push(build_lib_dir);
        }
        Self {
            program_name,
            python_home,
            search_paths,
        }
    }

    pub fn with_search_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.search_paths.push(path.into());
        self
    }

    pub fn with_search_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.search_paths.extend(paths);
        self
    }

    pub fn python_home(&self) -> &Path {
        &self.python_home
    }

    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace crate should live under crates/ in the repo root")
        .to_path_buf()
}

pub fn vendored_python_home(repo_root: &Path) -> PathBuf {
    std::env::var_os("CPYTHON_SOURCE_DIR")
        .or_else(|| option_env!("CPYTHON_SOURCE_DIR").map(Into::into))
        .map(|path| repo_root.join(path))
        .unwrap_or_else(|| repo_root.join("vendor").join("cpython"))
}

pub fn vendored_python_build_lib_dir(python_build: &Path) -> Option<PathBuf> {
    let pybuilddir = python_build.join("pybuilddir.txt");
    if let Ok(raw) = std::fs::read_to_string(pybuilddir) {
        let relative = raw.trim();
        if !relative.is_empty() {
            return Some(python_build.join(relative));
        }
    }

    let build_dir = python_build.join("build");
    let entries = std::fs::read_dir(build_dir).ok()?;
    for entry in entries {
        let path = entry.ok()?.path();
        if path.is_dir()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lib."))
        {
            return Some(path);
        }
    }
    None
}

fn test_extension_staging_dir(artifact_dir: &Path) -> Result<PathBuf, String> {
    let staging_dir = artifact_dir.join("test-ext");
    let source_ext = artifact_dir.join("lib_soac_ext.so");
    let staged_ext = staging_dir.join("_soac_ext.so");
    if !source_ext.is_file() {
        return Err(format!(
            "matching test extension not found at {}; build soac_pyo3 in this Cargo target/profile before embedded runtime tests",
            source_ext.display()
        ));
    }
    std::fs::create_dir_all(&staging_dir).map_err(|err| {
        format!(
            "failed to create test extension staging dir {}: {err}",
            staging_dir.display()
        )
    })?;
    stage_extension_file(&source_ext, &staged_ext)?;
    Ok(staging_dir)
}

#[cfg(unix)]
fn stage_extension_file(source_ext: &Path, staged_ext: &Path) -> Result<(), String> {
    let needs_symlink = std::fs::read_link(staged_ext)
        .map(|target| target != source_ext)
        .unwrap_or(true);
    if !needs_symlink {
        return Ok(());
    }
    if std::fs::symlink_metadata(staged_ext).is_ok() {
        std::fs::remove_file(staged_ext).map_err(|err| {
            format!(
                "failed to remove stale staged extension {}: {err}",
                staged_ext.display()
            )
        })?;
    }
    std::os::unix::fs::symlink(source_ext, staged_ext).map_err(|err| {
        format!(
            "failed to symlink staged extension {} -> {}: {err}",
            staged_ext.display(),
            source_ext.display()
        )
    })
}

#[cfg(not(unix))]
fn stage_extension_file(source_ext: &Path, staged_ext: &Path) -> Result<(), String> {
    std::fs::copy(source_ext, staged_ext)
        .map(|_| ())
        .map_err(|err| {
            format!(
                "failed to copy staged extension {} -> {}: {err}",
                source_ext.display(),
                staged_ext.display()
            )
        })
}

pub fn test_python_config(program_name: impl Into<String>) -> Result<EmbeddedPythonConfig, String> {
    let repo_root = repo_root();
    test_python_config_with_artifacts(
        &repo_root,
        program_name.into(),
        Path::new(env!("SOAC_TEST_ARTIFACT_DIR")),
    )
}

fn test_python_config_with_artifacts(
    repo_root: &Path,
    program_name: String,
    artifact_dir: &Path,
) -> Result<EmbeddedPythonConfig, String> {
    let staging_dir = test_extension_staging_dir(artifact_dir)?;
    Ok(EmbeddedPythonConfig::vendored(&repo_root, program_name)
        .with_search_path(repo_root.join("soac_py").join("src"))
        .with_search_path(staging_dir))
}

pub fn initialize_vendored_python(program_name: impl Into<String>) -> Result<(), String> {
    let config = EmbeddedPythonConfig::vendored(repo_root(), program_name);
    initialize_python_and_ensure_sys_path(&config)
}

pub fn initialize_test_python(program_name: impl Into<String>) -> Result<(), String> {
    let config = test_python_config(program_name)?;
    initialize_python_and_ensure_sys_path(&config)
}

pub fn initialize_python_and_ensure_sys_path(config: &EmbeddedPythonConfig) -> Result<(), String> {
    initialize_python(config)?;
    Python::attach(|py| ensure_sys_path_entries(py, config.search_paths()))
}

pub fn initialize_python(config: &EmbeddedPythonConfig) -> Result<(), String> {
    static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT_RESULT
        .get_or_init(|| initialize_python_once(config))
        .clone()
}

fn initialize_python_once(config: &EmbeddedPythonConfig) -> Result<(), String> {
    unsafe {
        if ffi::Py_IsInitialized() != 0 {
            return Ok(());
        }

        let path_config = LeakedPythonPathConfig::from_config(config)?;
        install_pre_init_path_config(&path_config);
        PYTHON_PATH_CONFIG
            .set(path_config)
            .map_err(|_| "embedded Python path config was already installed".to_string())?;
        Python::initialize();
    }
    Ok(())
}

#[allow(deprecated)]
unsafe fn install_pre_init_path_config(path_config: &LeakedPythonPathConfig) {
    // PyConfig would be the modern API, but the public struct layout exposed
    // through PyO3 can lag vendored CPython. These pre-init APIs avoid
    // process-environment mutation without depending on PyConfig field offsets.
    unsafe {
        ffi::Py_SetProgramName(path_config.program_name.as_ptr());
        ffi::Py_SetPythonHome(path_config.python_home.as_ptr());
        Py_SetPath(path_config.python_path.as_ptr());
    }
}

unsafe extern "C" {
    fn Py_SetPath(arg1: *const wchar_t);
}

pub fn ensure_sys_path_entries(py: Python<'_>, paths: &[PathBuf]) -> Result<(), String> {
    let sys_path = py
        .import("sys")
        .map_err(|err| err.to_string())?
        .getattr("path")
        .map_err(|err| err.to_string())?;
    for path in paths.iter().rev() {
        let path = path.to_string_lossy().into_owned();
        let present = sys_path
            .call_method1("__contains__", (path.as_str(),))
            .map_err(|err| err.to_string())?
            .extract::<bool>()
            .map_err(|err| err.to_string())?;
        if !present {
            sys_path
                .call_method1("insert", (0, path.as_str()))
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

static PYTHON_PATH_CONFIG: OnceLock<LeakedPythonPathConfig> = OnceLock::new();

#[derive(Debug)]
struct LeakedPythonPathConfig {
    program_name: DecodedWideString,
    python_home: DecodedWideString,
    python_path: DecodedWideString,
}

impl LeakedPythonPathConfig {
    fn from_config(config: &EmbeddedPythonConfig) -> Result<Self, String> {
        let python_path = std::env::join_paths(config.search_paths())
            .map_err(|err| format!("failed to join Python search paths: {err}"))?;
        Ok(Self {
            program_name: DecodedWideString::from_str(config.program_name.as_str())?,
            python_home: DecodedWideString::from_path(config.python_home())?,
            python_path: DecodedWideString::from_os_str(&python_path)?,
        })
    }
}

unsafe impl Send for LeakedPythonPathConfig {}
unsafe impl Sync for LeakedPythonPathConfig {}

#[derive(Debug)]
struct DecodedWideString {
    ptr: *mut wchar_t,
}

impl DecodedWideString {
    fn from_str(value: &str) -> Result<Self, String> {
        let raw = CString::new(value)
            .map_err(|_| "Python config string contains an interior NUL byte".to_string())?;
        Self::from_cstring(&raw, value)
    }

    fn from_path(path: &Path) -> Result<Self, String> {
        let raw = path_to_cstring(path)?;
        Self::from_cstring(&raw, path.display().to_string().as_str())
    }

    fn from_os_str(value: &std::ffi::OsStr) -> Result<Self, String> {
        let raw = os_str_to_cstring(value)
            .map_err(|_| "Python path contains an interior NUL byte".to_string())?;
        Self::from_cstring(&raw, value.to_string_lossy().as_ref())
    }

    fn from_cstring(raw: &CString, display: &str) -> Result<Self, String> {
        let ptr = unsafe { ffi::Py_DecodeLocale(raw.as_ptr(), ptr::null_mut()) };
        if ptr.is_null() {
            return Err(format!("failed to decode Python config string {display}"));
        }
        Ok(Self { ptr })
    }

    fn as_ptr(&self) -> *const wchar_t {
        self.ptr
    }
}

impl Drop for DecodedWideString {
    fn drop(&mut self) {
        unsafe {
            ffi::PyMem_RawFree(self.ptr.cast());
        }
    }
}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> Result<CString, String> {
    os_str_to_cstring(path.as_os_str()).map_err(|_| {
        format!(
            "Python path contains an interior NUL byte: {}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> Result<CString, String> {
    os_str_to_cstring(path.as_os_str()).map_err(|_| {
        format!(
            "Python path contains an interior NUL byte: {}",
            path.display()
        )
    })
}

#[cfg(unix)]
fn os_str_to_cstring(value: &std::ffi::OsStr) -> Result<CString, std::ffi::NulError> {
    CString::new(value.as_bytes())
}

#[cfg(not(unix))]
fn os_str_to_cstring(value: &std::ffi::OsStr) -> Result<CString, std::ffi::NulError> {
    CString::new(value.to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ArtifactFixture(PathBuf);

    impl ArtifactFixture {
        fn new() -> Self {
            static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "soac-test-extension-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for ArtifactFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn test_extension_stages_from_the_actual_cargo_artifact_directory() {
        let fixture = ArtifactFixture::new();
        let artifacts = fixture.0.join("external-target/debug");
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("lib_soac_ext.so"), b"matched extension").unwrap();

        let staging = test_extension_staging_dir(&artifacts).unwrap();

        assert_eq!(staging, artifacts.join("test-ext"));
        assert_eq!(
            std::fs::read(staging.join("_soac_ext.so")).unwrap(),
            b"matched extension"
        );
    }

    #[test]
    fn test_extension_missing_source_does_not_accept_a_stale_staged_library() {
        let fixture = ArtifactFixture::new();
        let staging = fixture.0.join("test-ext");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("_soac_ext.so"), b"stale extension").unwrap();

        assert!(test_extension_staging_dir(&fixture.0).is_err());
        assert_eq!(
            std::fs::read(staging.join("_soac_ext.so")).unwrap(),
            b"stale extension",
            "a rejected setup must not destroy an earlier artifact"
        );
    }

    #[test]
    fn test_extension_replaces_a_stale_link_with_the_matched_artifact() {
        let fixture = ArtifactFixture::new();
        let old = fixture.0.join("old.so");
        let staging = fixture.0.join("test-ext");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(&old, b"old extension").unwrap();
        stage_extension_file(&old, &staging.join("_soac_ext.so")).unwrap();
        std::fs::write(fixture.0.join("lib_soac_ext.so"), b"current extension").unwrap();

        let actual = test_extension_staging_dir(&fixture.0).unwrap();

        assert_eq!(actual, staging);
        assert_eq!(
            std::fs::read(actual.join("_soac_ext.so")).unwrap(),
            b"current extension"
        );
    }

    #[test]
    fn out_of_tree_config_keeps_shared_stdlib_and_guest_extension_paths_separate() {
        let temp = std::env::temp_dir().join(format!(
            "soac-cpython-separated-build-{}",
            std::process::id()
        ));
        let root = temp.join("shared");
        let build = temp.join("guest-build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("pybuilddir.txt"), "build/lib.test-python\n").unwrap();

        let config = EmbeddedPythonConfig::vendored_with_build_dir(
            &root.join("vendor/cpython"),
            "soac-test".into(),
            &build,
        );

        assert_eq!(config.python_home(), root.join("vendor/cpython"));
        assert_eq!(
            config.search_paths(),
            [
                root.join("vendor/cpython/Lib"),
                build.join("build/lib.test-python")
            ]
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn vendored_config_contains_stdlib_path() {
        let root = repo_root();
        let config = EmbeddedPythonConfig::vendored(&root, "soac-test");

        assert_eq!(config.python_home(), vendored_python_home(&root));
        assert!(
            config
                .search_paths()
                .contains(&config.python_home.join("Lib"))
        );
    }

    #[test]
    fn test_config_contains_soac_sources_and_staging_dir() {
        let fixture = ArtifactFixture::new();
        let root = fixture.0.join("source");
        let artifacts = fixture.0.join("artifacts/debug");
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("lib_soac_ext.so"), b"extension fixture").unwrap();
        let config =
            test_python_config_with_artifacts(&root, "soac-test".into(), &artifacts).unwrap();

        assert!(config.search_paths().contains(&root.join("soac_py/src")));
        assert!(config.search_paths().contains(&artifacts.join("test-ext")));
    }

    #[test]
    fn compiled_artifact_directory_matches_this_test_binary() {
        let executable = std::env::current_exe().unwrap();
        let artifacts = executable.parent().unwrap().parent().unwrap();
        assert_eq!(
            std::fs::canonicalize(env!("SOAC_TEST_ARTIFACT_DIR")).unwrap(),
            std::fs::canonicalize(artifacts).unwrap(),
        );
    }
}
